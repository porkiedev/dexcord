#![allow(unused)]

pub mod discord_protocols {
    pub mod users {
        include!(concat!(env!("OUT_DIR"), "/discord_protocols.users.rs"));
    }
}
mod dexcom;
mod discord;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{env::current_exe, fs::File, time::Duration};
use base64::Engine;
use discord_protocols::users::*;
use preloaded_user_settings::{CustomStatus, StatusSettings};
use prost::Message;
use tracing::{debug, error, info, trace, warn, Level};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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

    // Create the API instances
    let discord_api = discord::Api::new(&config.discord_token).await;
    let mut dexcom_api = dexcom::Api::new(&config.dexcom_username, &config.dexcom_password).await.unwrap();

    // A flag used to update the status immediately on the first loop iteration
    let mut loop_has_started = false;

    loop {
        
        // Sleep for 5 minutes. This doesn't apply to the first loop iteration since that's the first one
        if loop_has_started {
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
        // Update the loop flag since we just started
        loop_has_started = true;

        // Get a blood sugar measurement
        let status_string = match dexcom_api.get_latest_glucose().await {
            Ok(measurement) => {
                // If the API returned an empty response, log a warning and continue
                if measurement.is_none() {
                    warn!("The API didn't return a glucose measurement");
                    continue;
                }
                // Shadow the measurement variable
                let measurement = measurement.unwrap();
                trace!("Successfully got glucose measurement: {}", measurement.value);
                // Return the status string
                format_status(measurement.value)
            },
            Err(e) => {

                // If the session expired, just continue
                if let Some(&dexcom::Error::SessionInvalid) = e.downcast_ref::<dexcom::Error>() {
                    debug!("The dexcom session ID expired. Retrying with a new session ID...");
                    // Reset the loop flag so we instantly retry
                    loop_has_started = false;
                    continue;
                } else {
                    error!("Failed to get latest glucose measurement: {e:?}");
                    "Tell me to change my cgm".to_string()
                }
            }
        };

        // Log a warning if the status update failed
        if let Err(e) = discord_api.set_status(&status_string).await {
            warn!("Failed to update discord account status: {e:?}");
            continue;
        }
    }

}

fn format_status(value: u32) -> String {
    if (40..60).contains(&value) {
        format!("I'm in sugar withdrawls, send help ({value} mg/dL)")
    }
    else if (60..80).contains(&value) {
        format!("Tell me to eat something, I'm a little low ({value} mg/dL)")
    }
    else if (80..200).contains(&value) {
        format!("We chillin ({value} mg/dL)")
    }
    else if (200..300).contains(&value) {
        format!("I'm a little high, tell me to do some pushups ({value} mg/dL)")
    }
    else {
        format!("I'm currently ODing on sugar, send help ({value} mg/dL)")
    }
}

/// The application configuration
#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    dexcom_username: String,
    dexcom_password: String,
    discord_token: String
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

        // If the file doesn't exist or we can't open it, return None (i.e. create a new config)
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
