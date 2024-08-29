//
// An interface to the (undocumented) Dexcom Share API
//

use std::{env::current_exe, fs::File, path::PathBuf};
use serde::{Deserialize, Serialize};
use anyhow::{Context, Result};
use tracing::{debug, error, warn};

/// The application ID
const APPLICATION_ID: &str = "d89443d2-327c-4a6f-89e5-496bbb0317db";
/// The URL to fetch the account ID
const ACCOUNT_ID_URL: &str = "https://share2.dexcom.com/ShareWebServices/Services/General/AuthenticatePublisherAccount";
/// The URL to fetch the session ID
const SESSION_ID_URL: &str = "https://share2.dexcom.com/ShareWebServices/Services/General/LoginPublisherAccountById";
/// The URL to fetch glucose measurements
const MEASURE_GLUCOSE_URL: &str = "https://share2.dexcom.com/ShareWebServices/Services/Publisher/ReadPublisherLatestGlucoseValues";
/// The oldest glucose measurement to fetch
const DEFAULT_MINUTES: usize = 60;
/// The maximum number of glucose measurements to fetch
const DEFAULT_MAX_COUNT: usize = 1;

#[derive(Debug)]
pub struct Api {
    /// The HTTP client
    client: reqwest::Client,
    /// The password of the account
    password: String,
    /// Cachable information regarding the API connection
    cache: ApiCache
}
impl Api {
    pub async fn new(username: &str, password: &str) -> Result<Self> {

        // Ensure the username and password are not empty
        if username.is_empty() { Err(Error::ArgUsername)? };
        if password.is_empty() { Err(Error::ArgPassword)? };

        // Create the HTTP client
        let mut client = reqwest::Client::new();

        // Try to load the cache if it exists, otherwise create a new one
        let mut should_refresh_cache = false;
        let cache = ApiCache::try_load_cache(username).unwrap_or_else(|| {
            // Update the cache refresh flag
            should_refresh_cache = true;
            ApiCache::default()
        });

        // Create an instance of self
        let mut s = Self {
            client,
            password: password.to_string(),
            cache
        };

        // Update the username
        s.cache.username = username.to_string();

        // Update the account and session ID cache if necessary
        if should_refresh_cache {
            s.cache.account_id = s.get_account_id().await?;
            s.cache.session_id = s.get_session_id().await?;
            // Save the cache
            s.cache.save();
        }

        Ok(s)
    }

    /// Queries the API for the ID of the account
    async fn get_account_id(&self) -> Result<String> {
        debug!("Getting account ID...");

        // Send the request to the API and get the response body
        let body = self.client.post(ACCOUNT_ID_URL)
        .json(&AccountIdRequest {
            username: &self.cache.username,
            password: &self.password,
            application_id: APPLICATION_ID
        })
        .send().await?
        .text().await?;

        // Parse the response body into an account ID string
        if let Ok(account_id) = serde_json::from_str::<String>(&body) {
            Ok(account_id)
        }
        // Parse the response body into an error
        else if let Ok(e) = serde_json::from_str::<ErrorResponse>(&body) {
            error!("Failed to get account ID: {e:?}");
            Err(e.code)?
        }
        // Parse the response body into an unknown error
        else {
            Err(Error::Unknown(body))?
        }
    }

    async fn get_session_id(&self) -> Result<String> {
        debug!("Getting session ID...");

        // Send the request to the API and get the response body
        let body = self.client.post(SESSION_ID_URL)
        .json(&SessionIdRequest {
            account_id: &self.cache.account_id,
            password: &self.password,
            application_id: APPLICATION_ID
        })
        .send().await?
        .text().await?;

        // Parse the response body into a session ID string
        if let Ok(session_id) = serde_json::from_str::<String>(&body) {
            Ok(session_id)
        }
        // Parse the response body into an error
        else if let Ok(e) = serde_json::from_str::<ErrorResponse>(&body) {
            error!("Failed to get session ID: {e:?}");
            Err(e.code)?
        }
        // Parse the response body into an unknown error
        else {
            Err(Error::Unknown(body))?
        }
    }

    pub async fn get_latest_glucose(&mut self) -> Result<Option<GlucoseMeasurement>> {

        // Send the request to the API and get the response body
        let body = self.client.post(MEASURE_GLUCOSE_URL)
        .json(&MeasureGlucoseRequest {
            session_id: &self.cache.session_id,
            minutes: DEFAULT_MINUTES,
            max_count: DEFAULT_MAX_COUNT
        })
        .send().await?
        .text().await?;

        // Parse the response body into a session ID string
        if let Ok(mut response) = serde_json::from_str::<Vec<GlucoseMeasurement>>(&body) {
            if response.is_empty() {
                Ok(None)
            } else {
                Ok(Some(response.remove(0)))
            }
        }
        // Parse the response body into an error
        else if let Ok(e) = serde_json::from_str::<ErrorResponse>(&body) {

            // If the session ID just expired, try to renew it for the next request
            if let Error::SessionInvalid = e.code {
                self.cache.session_id = self.get_session_id().await?;
                // Save the cache
                self.cache.save();
            }

            error!("Failed to get glucose measurement: {e:?}");
            Err(e.code)?
        }
        // Parse the response body into an unknown error
        else {
            Err(Error::Unknown(body))?
        }
    }
}

/// Cachable information regarding the API. These are saved and fetched from the cache file.
/// 
/// - NOTE: This caches the username so we can hopefully detect if the targeted user has changed (thus requiring a cache refresh)
#[derive(Debug, Default, Serialize, Deserialize)]
struct ApiCache {
    /// The username of the account
    username: String,
    /// The ID of the account
    account_id: String,
    /// The ID of the session
    session_id: String
}
impl ApiCache {
    fn try_load_cache(username: &str) -> Option<Self> {
        debug!("Trying to load the API cache...");

        // Get the path to the cache file
        let path = {
            current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
            .join("api_cache.json")
        };
        // Open the cache file
        let file = File::open(path);

        // If the file doesn't exist or we can't open it, return None (i.e. create a new cache)
        if let Err(e) = file {
            warn!("Failed to open the API cache file: {e:?}");
            return None;
        }
        let file = file.unwrap();

        // Read the cache file
        let cached_s: Self = serde_json::from_reader(file)
        .context("The API cache is invalid (perhaps try deleting it)")
        .unwrap();

        // The username changed. The cache should be refreshed
        if cached_s.username != username {
            None
        }
        // The cache is still valid, so return it
        else {
            debug!("API cache is still valid");
            Some(cached_s)
        }
    }

    fn save(&self) {
        // Get the path to the cache file
        let path = {
            current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
            .join("api_cache.json")
        };
        // Create the cache file
        let file = File::create(path).unwrap();
        // Write self to the cache file
        serde_json::to_writer_pretty(file, &self)
        .context("Failed to write to the API cache file")
        .unwrap();
    }
}
impl Drop for ApiCache {
    fn drop(&mut self) {
        self.save();
    }
}

// API REQUESTS
/// The body for the account ID request
#[derive(Debug, Serialize)]
struct AccountIdRequest<'a> {
    /// The username of the account
    #[serde(rename = "accountName")]
    username: &'a str,
    /// The password of the account (scary password stuff)
    password: &'a str,
    /// The application ID
    #[serde(rename = "applicationId")]
    application_id: &'a str
}

/// The body for the session ID request
#[derive(Debug, Serialize)]
struct SessionIdRequest<'a> {
    /// The ID of the account
    #[serde(rename = "accountId")]
    account_id: &'a str,
    /// The password of the account (still scary password stuff)
    password: &'a str,
    /// The application ID
    #[serde(rename = "applicationId")]
    application_id: &'a str
}

/// The body for the measure glucose request
#[derive(Debug, Serialize)]
struct MeasureGlucoseRequest<'a> {
    /// The ID of the session
    #[serde(rename = "sessionId")]
    session_id: &'a str,
    /// How long ago should be look for glucose readings (in minutes)
    minutes: usize,
    /// How many readings should be returned at most?
    #[serde(rename = "maxCount")]
    max_count: usize
}

/// A single glucose measurement in the glucose readings response body
#[derive(Debug, Deserialize)]
pub struct GlucoseMeasurement {
    /// The date and time of the measurement
    #[serde(rename = "WT")]
    pub wt: String,
    /// The date and time of the measurement
    #[serde(rename = "ST")]
    pub st: String,
    /// The date and time of the measurement
    #[serde(rename = "DT")]
    pub dt: String,
    /// The glucose value
    #[serde(rename = "Value")]
    pub value: u32,
    /// The trend of the glucose value
    #[serde(rename = "Trend")]
    pub trend: String
}

/// An error response from the API
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    #[serde(rename = "Code")]
    code: Error,
    #[serde(rename = "Message")]
    message: String,
    #[serde(rename = "SubCode")]
    description: String,
    #[serde(rename = "TypeName")]
    type_name: String
}

#[derive(Debug, Deserialize, thiserror::Error)]
pub enum Error {
    #[serde(rename = "AccountPasswordInvalid")]
    #[error("Invalid username or password")]
    InvalidPassword,
    #[error("Maximum number of authentication attempts reached")]
    MaxAuthenticationAttemptsReached,
    #[serde(rename = "SessionIdNotFound")]
    #[error("Session ID not found")]
    SessionNotFound,
    #[serde(rename = "SessionNotValid")]
    #[error("Session ID not active or expired")]
    SessionInvalid,
    #[error("The username must not be empty")]
    ArgUsername,
    #[error("The password must not be empty")]
    ArgPassword,
    #[error("The maximum number of glucose measurement retries has been reached")]
    MaxRetriesReached,
    #[error("Encountered an unknown error: {0}")]
    Unknown(String)
}
