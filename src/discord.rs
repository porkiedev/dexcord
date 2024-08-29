//
// An interface to the (undocumented and dangerous) Discord user API
// It's dangerous because it's against Discord's ToS to automate user accounts (i.e. treat them like bots)
//

use std::time::{SystemTime, UNIX_EPOCH};
use base64::engine::general_purpose::STANDARD;
use anyhow::{Context, Result};
use base64::Engine;
use prost::Message;
use serde_json::json;
use tracing::{debug, error, trace};
use crate::{preloaded_user_settings::{CustomStatus, StatusSettings}, PreloadedUserSettings};

const PROTO_SETTINGS_URL: &str = "https://discord.com/api/v9/users/@me/settings-proto/1";

#[derive(Debug)]
pub struct Api {
    /// The HTTP client
    client: reqwest::Client,
    /// The token of the account
    token: String
}
impl Api {
    /// Create a new API instance
    pub async fn new(token: &str) -> Self {

        // Create the HTTP client
        // We spoof the user agent here to reduce our chances of being detected by discord
        let client = reqwest::ClientBuilder::default()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:129.0) Gecko/20100101 Firefox/129.0")
        .build()
        .context("Failed to create HTTP client for the discord API").unwrap();

        Self {
            client,
            token: token.to_string()
        }
    }

    /// Updates the status of the account with the provided string
    pub async fn set_status(&self, text: &str) -> Result<()> {

        // Create the status change packet
        let packet = PreloadedUserSettings {
            status: Some(StatusSettings {
                status: None,
                custom_status: Some(CustomStatus {
                    text: text.to_string(),
                    emoji_id: 0,
                    emoji_name: String::new(),
                    expires_at_ms: 0, // This implies the status is permanent
                    created_at_ms: get_epoch_ms(), // It works without this, but hopefully this will help trick the API into thinking we are mere mortals
                }),
                show_current_game: None,
                status_expires_at_ms: 0,
            }),
            ..Default::default()
        };

        // Convert the packet into a base64 string that can be sent to the API
        let packet_b64 = STANDARD.encode(packet.encode_to_vec());

        // Send the request to the API
        let response = self.client.patch(PROTO_SETTINGS_URL)
        .header("Authorization", &self.token)
        .json(&json!({ "settings": packet_b64 }))
        .send().await?;

        // The status change was successful
        if response.status().is_success() {
            trace!("Updated status to '{}' successfully", text);
            Ok(())
        }
        // The status change failed
        else {
            let body = response.text().await?;
            error!("Failed to update status to '{}': {}", text, body);
            Err(Error::Unknown(body))?
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Received an unknown error: {0}")]
    Unknown(String)
}

/// Returns the current unix epoch in milliseconds
fn get_epoch_ms() -> u64 {
    SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_millis() as u64
}
