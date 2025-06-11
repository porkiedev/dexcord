#![allow(unused)]

pub mod discord_protocols {
    pub mod users {
        include!(concat!(env!("OUT_DIR"), "/discord_protos.discord_users.v1.rs"));
    }
}
mod dexcom;
mod discord;
mod database;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{env::current_exe, fs::File, time::Duration};
use base64::Engine;
use discord_protocols::users::*;
use preloaded_user_settings::{CustomStatus, StatusSettings};
use prost::Message;
use tracing::{debug, error, info, trace, warn, Level};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use crate::dexcom::GlucoseMeasurement;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger
    let filter = tracing_subscriber::filter::Targets::new()
        .with_target(module_path!(), Level::TRACE); // Log only this module at TRACE level
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    // Load the config
    let config = Config::new();
    // Load the cache
    let mut cache = Cache::new();

    // Create the API instances
    let discord_api = discord::Api::new(&config.discord_token).await;
    let dexcom_api = dexcom::Api::new(&config).await?;
    let db = database::Database::new(&config);

    // How long (in seconds) should we wait between each loop iteration. This is set to 5 minutes by default but
    // may be temporarily changed to something shorter if we need to query the dexcom API for a new session ID
    let mut loop_wait_time = 0;

    loop {
        // Sleep for the specified amount of time
        tokio::time::sleep(Duration::from_secs(loop_wait_time)).await;
        // Reset the wait time to 5 minutes
        loop_wait_time = 240;

        // Get a blood sugar measurement
        let status_string = match dexcom_api.get_latest_glucose(&config, &mut cache).await {
            Ok(measurement) => {

                // Get the measurement if it exists
                let Some(measurement) = measurement else {
                    warn!("The API didn't return a glucose measurement");
                    continue;
                };

                trace!("Successfully got glucose measurement: {}", measurement.value);
                // Format the status string
                let status_string = format_status(measurement.value);
                // Insert the glucose measurement into the database, ignoring any errors
                // as the database logs them, and we don't want to crash the app
                let _ = db.insert_glucose(&config, &mut cache, measurement).await;

                // Return the status string
                status_string
            },
            Err(e) => {

                // The dexcom module will log the error for us, so we just need to retry
                debug!("Retrying in 10 seconds...");
                loop_wait_time = 10;
                continue;
            }
        };
        
        // Log a warning if the status update failed
        if let Err(e) = discord_api.set_status(&status_string).await {
            warn!("Failed to update discord account status: {e:?}");
            continue;
        }
    }

}

/// Formats a glucose value into a string that can be used to update the discord account status
fn format_status(value: u32) -> String {
    match value {
        ..40 => format!("I'm currently dying, send help ({value} mg/dL)"),
        40..60 => format!("I'm in sugar withdrawls, send help ({value} mg/dL)"),
        60..80 => format!("Tell me to eat something, I'm a little low ({value} mg/dL)"),
        80..200 => format!("We chillin ({value} mg/dL)"),
        200..300 => format!("I'm a little high, tell me to do some pushups ({value} mg/dL)"),
        300.. => format!("I'm currently ODing on sugar, send help ({value} mg/dL)")
    }
}

/// The application configuration
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub dexcom_username: String,
    pub dexcom_password: String,
    pub discord_token: String,
    pub database_url: String,
    pub database_namespace: String,
    pub database_name: String,
    pub database_username: String,
    pub database_password: String
}
impl Config {
    /// Returns the existsing config file.
    ///
    /// - NOTE: If there is no config file, it will create a new one and panic.
    fn new() -> Self {
        debug!("Trying to load the config file...");

        // Get the path to the config file
        let path = {
            current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
            .join("config.json")
        };
        // Open the config file
        let file = File::open(path);

        // If the file doesn't exist, or we can't open it, return None (i.e. create a new config)
        if let Err(e) = file {
            warn!("Failed to open the config file: {e:?}");
            info!("Created a new config file. Please edit it and restart the program.");
            // Save the default config and panic
            Self::default().save();
            panic!("Read the above message");
        }
        let file = file.unwrap();

        // Read the config file
        let cached_c: Self = serde_json::from_reader(file)
        .context("The config file is invalid (perhaps try deleting it)")
        .unwrap();

        cached_c
    }

    fn save(&self) {
        // Get the path to the config file
        let path = {
            current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
            .join("config.json")
        };
        // Create the config file
        let file = File::create(path).unwrap();
        // Write self to the config file
        serde_json::to_writer_pretty(file, &self)
        .context("Failed to write to the config file")
        .unwrap();
    }
}

/// The global application cache
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    /// The ID of the provided dexcom account
    pub dexcom_account_id: String,
    /// The ID of the latest dexcom session
    pub dexcom_session_id: String,
    /// The latest auth token of the database user
    pub db_auth_token: String,
    /// Pending glucose measurements that need to be inserted into the database.
    /// This should generally contain no more than 1 measurement.
    /// If it does, you may have issues with database authentication.
    pub pending_measurements: Vec<GlucoseMeasurement>
}
impl Cache {
    /// Returns the existing cache file, or creates a new one if it doesn't exist or can't be opened
    fn new() -> Self {
        debug!("Trying to load the cache...");

        // Get the path to the cache file
        let path = {
            current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
                .join("cache.json")
        };
        // Open the cache file
        let file = File::open(path);

        // If the file doesn't exist, or we can't open it, return a new cache
        let file = match file {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to open the cache file: {e:?}");
                return Self::default();
            }
        };

        // Deserialize and return the cache file
        serde_json::from_reader(file)
            .context("The cache file is invalid (perhaps try deleting it)")
            .unwrap()
    }

    /// Saves the cache file
    fn save(&self) {
        // Get the path to the cache file
        let path = {
            current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
                .join("cache.json")
        };

        // Create the cache file
        let file = match File::create(path) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create the cache file: {e:?}");
                return;
            }
        };
        
        // Write self to the cache file
        if let Err(e) = serde_json::to_writer_pretty(file, &self) {
            error!("Failed to write to the cache file: {e:?}");
        }
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        self.save();
    }
}
