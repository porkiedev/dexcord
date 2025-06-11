//
// An interface to the (undocumented) Dexcom Share API
//

use std::{env::current_exe, fs::File, path::PathBuf};
use serde::{Deserialize, Deserializer, Serialize};
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::{Cache, Config};

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
    client: reqwest::Client
}
impl Api {
    pub async fn new(config: &Config) -> Result<Self> {

        // Ensure the username and password are not empty
        if config.dexcom_username.is_empty() { Err(Error::ArgUsername)? };
        if config.dexcom_password.is_empty() { Err(Error::ArgPassword)? };

        // Create the HTTP client
        let mut client = reqwest::Client::new();

        // Create an instance of self
        let mut s = Self {
            client
        };

        Ok(s)
    }

    /// Queries the API for the ID of the account
    async fn get_account_id(&self, config: &Config) -> Result<String> {
        debug!("Getting account ID...");

        // Send the request to the API and get the response body
        let body = self.client.post(ACCOUNT_ID_URL)
        .json(&AccountIdRequest {
            username: &config.dexcom_username,
            password: &config.dexcom_password,
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

    /// Queries the API for a new session ID
    async fn get_session_id(&self, config: &Config, account_id: &str) -> Result<String> {
        debug!("Getting session ID...");

        // Send the request to the API and get the response body
        let body = self.client.post(SESSION_ID_URL)
        .json(&SessionIdRequest {
            account_id,
            password: &config.dexcom_password,
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

    /// Updates the cache with the dexcom account and session ID
    async fn update_cache(&self, config: &Config, cache: &mut Cache) -> Result<()> {
        // Update the cache with a new account and session ID
        cache.dexcom_account_id = self.get_account_id(config).await?;
        cache.dexcom_session_id = self.get_session_id(config, &cache.dexcom_account_id).await?;
        cache.save();
        Ok(())
    }

    pub async fn get_latest_glucose(&self, config: &Config, cache: &mut Cache) -> Result<Option<GlucoseMeasurement>> {

        // Update the cache if the account or session ID is missing
        if cache.dexcom_account_id.is_empty() | cache.dexcom_session_id.is_empty() {
            self.update_cache(config, cache).await?;
        }

        // Send the request to the API and get the response body
        let body = self.client.post(MEASURE_GLUCOSE_URL)
        .json(&MeasureGlucoseRequest {
            session_id: &cache.dexcom_session_id,
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

            if let Error::SessionInvalid | Error::SessionNotFound = e.code {
                debug!("Session ID expired or invalid. Refreshing the dexcom account and session ID...");
                self.update_cache(config, cache).await?;
            } else {
                error!("Failed to get glucose measurement: {e:?}");
            }

            Err(e.code)?
        }
        // Parse the response body into an unknown error
        else {
            error!("Failed to get glucose measurement: {body:?}");
            Err(Error::Unknown(body))?
        }
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
#[derive(Debug, Serialize, Deserialize)]
pub struct GlucoseMeasurement {
    /// The epoch of the measurement
    #[serde(rename = "WT", deserialize_with = "deserialize_dexcom_date_string")]
    pub wt: u64,
    /// The epoch of the measurement
    #[serde(rename = "ST", deserialize_with = "deserialize_dexcom_date_string")]
    pub st: u64,
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

/// Used to deserialize various types into a single u64, intended to represent a unix timestamp in milliseconds.
/// This is primarily used to deserialize the dexcom API response which returns something like `Date(1597363200000)`
pub fn deserialize_dexcom_date_string<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    struct DateVisitor;
    impl serde::de::Visitor<'_> for DateVisitor {
        type Value = u64;
        
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("Expected a u64, a string 'Date(u64)', or a string 'u64'")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
            Ok(value)
        }

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if let Some(stripped) = s.strip_prefix("Date(").and_then(|x| x.strip_suffix(")")) {
                stripped.parse::<u64>().map_err(E::custom)
            } else {
                s.parse::<u64>().map_err(E::custom)
            }
        }

        fn visit_string<E>(self, s: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            self.visit_str(&s)
        }
    }

    deserializer.deserialize_any(DateVisitor)
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
