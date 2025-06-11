//
// Contains code used to log glucose measurements to a surrealdb database
//

use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::StatusCode;
use tracing::{debug, error, trace, warn};
use crate::{Cache, Config};
use crate::dexcom::GlucoseMeasurement;

/// The surrealdb database interface
#[derive(Debug)]
pub struct Database {
    /// The HTTP client
    client: reqwest::Client,
    /// The HTTP headers to use for requests excluding the `/signin` endpoint
    headers: HeaderMap
}
impl Database {
    /// Create a new database instance
    pub fn new(config: &Config) -> Self {

        // Create the HTTP client for the database endpoint
        let client = reqwest::Client::builder()
            .https_only(true)
            .build()
            .expect("Failed to create database HTTP client");

        // Create the HTTP headers for the database endpoint
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", HeaderValue::from_static("application/json"));
        headers.insert("Surreal-NS", HeaderValue::from_str(&config.database_namespace)
            .expect("Failed to create Surreal-NS header"));
        headers.insert("Surreal-DB",HeaderValue::from_str(&config.database_name)
            .expect("Failed to create Surreal-DB header"));

        Self {
            client,
            headers,
        }
    }

    /// Authenticates with the database and updates the auth token
    pub async fn authenticate(&self, config: &Config, cache: &mut Cache) -> Result<()> {
        trace!("Authenticating with the database...");

        // Send the auth request to the database and deserialize the response
        let res = self.client
            .post(format!("{}/signin", config.database_url))
            .header("Accept", "application/json")
            .json(&AuthorizationRequest {
                namespace: &config.database_namespace,
                database: &config.database_name,
                username: &config.database_username,
                password: &config.database_password
            })
            .send()
            .await?
            .text()
            .await?;
        
        // Deserialize the response body into an authorization response
        let Ok(json_res) = serde_json::from_str::<AuthorizationResponse>(&res) else {
            error!("Failed to authenticate with the database (raw response body): {res}");
            Err(Error::Authentication)?
        };
        
        // Update the cached auth token if successful
        match json_res.token {
            Some(token) => {
                debug!("Successfully authenticated with the database");
                cache.db_auth_token = token;
                // Save the cache
                cache.save();
            },
            None => {
                error!("Failed to authenticate with the database (no token in response body): {json_res:?}");
                Err(Error::Authentication)?
            }
        }

        Ok(())
    }

    /// Inserts a glucose measurement into the database
    pub async fn insert_glucose(&self, config: &Config, cache: &mut Cache, measurement: GlucoseMeasurement) -> Result<()> {

        // Add the measurement to the pending measurements queue
        cache.pending_measurements.push(measurement);
        // Save the cache in case the database credentials are incorrect and the app needs to be restarted so we don't lose measurements
        cache.save();
        
        // Create the SQL string to insert the measurements into the database
        let mut sql = String::new();
        for measurement in &cache.pending_measurements {
            sql.push_str(&format!("INSERT INTO measurements {{ id: rand::ulid(), glucose: {}, measured_at: time::from::millis({}) }};",
                                  measurement.value,
                                  measurement.st
            ));
            sql.push('\n');
        }

        // If there are too many pending measurements, drop the oldest ones to avoid "leaking" memory and storage space
        if cache.pending_measurements.len() > 1024 {
            warn!("There number of pending measurements has exceeded the maximum of 1024. This should never happen unless database authentication is failing. Old measurements will be dropped.");
            cache.pending_measurements.drain(1024..);
        }

        // Insert the glucose measurement(s) into the database
        let res = self.client.post(format!("{}/sql", config.database_url))
            .bearer_auth(&cache.db_auth_token)
            .headers(self.headers.clone())
            .body(sql)
            .send().await?;

        // Ensure the response was successful
        let value = match res.status() {
            StatusCode::OK => {
                debug!("Successfully inserted ({}) glucose measurement(s) into database",
                    cache.pending_measurements.len());
                // Clear the pending measurements as they have been successfully inserted
                cache.pending_measurements.clear();
                // Save the cache
                cache.save();
                Ok(())
            },
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                debug!("Failed to insert glucose measurement(s) into the database due to an authorization error (401 or 403). \
                Refreshing the auth token...");

                // Refresh the auth token for the next insert attempt
                self.authenticate(config, cache).await?;
                Ok(())
            },
            _ => {
                let e = Error::Unknown(res.text().await?);
                error!("Failed to insert glucose measurement(s) into the database: {e:?}");
                Err(e)
            }
        };
        
        // Return the result
        Ok(value?)
    }
}

/// The request body send to the `POST /signin` endpoint. Must be serialized into JSON
#[derive(Debug, serde::Serialize)]
struct AuthorizationRequest<'a> {
    /// The namespace you want to use
    #[serde(rename = "ns")]
    namespace: &'a str,
    /// The database you want to use
    #[serde(rename = "db")]
    database: &'a str,
    /// The username of your account
    #[serde(rename = "user")]
    username: &'a str,
    /// The password of your account
    #[serde(rename = "pass")]
    password: &'a str
}

/// The response body received from the `POST /signin` endpoint. Must be deserialized from JSON
#[derive(Debug, serde::Deserialize)]
struct AuthorizationResponse {
    /// The status code of the response
    code: u32,
    /// The message of the response
    details: String,
    /// A token that can be used to authenticate future requests, if successful. This is a JWT token
    token: Option<String>,
    /// The refresh token (not supported by included for completeness)
    refresh: Option<String>
}

/// The error types returned by the database
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Authentication failed
    #[error("Failed to authenticate with the database")]
    Authentication,
    #[error("Encountered an unknown error from the database: {0}")]
    Unknown(String)
}
