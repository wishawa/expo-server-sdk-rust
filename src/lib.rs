//! # Expo Push Notification Rust Client
//!
//! The Expo Push Notification client provides a way for you to send push notifications to users of
//! your mobile app using the Expo push notification services. For more details on the Expo push
//! notification service, go [here]
//!
//! [here]: https://docs.expo.io/versions/latest/guides/push-notifications
//!
//! ## Example: Sending a push notification
//!
//! ```
//! # use expo_server_sdk::{ExpoNotificationsClient, message::*};
//! # use std::str::FromStr;
//! # tokio_test::block_on(async {
//! let token = PushToken::from_str("ExpoPushToken[my-token]").unwrap();
//! let mut msg = PushMessage::new(token).body("test notification");
//!
//! let client = ExpoNotificationsClient::new();
//! let result = client.send_push_notification(&msg).await;
//!
//! if let Ok(result) = result {
//!     println!("Push Notification Response: \n \n {:#?}", result);
//! }
//! # })
//! ```

pub mod error;
mod gzip_policy;
pub mod message;
pub mod response;
pub use gzip_policy::GzipPolicy;
use serde::Serialize;

use std::{borrow::Borrow, collections::HashMap};

use error::ExpoNotificationError;
use message::PushMessage;
use reqwest::{
    header::{HeaderValue, ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE},
    Url,
};
use response::{PushReceipt, PushReceiptId, PushResponse, PushTicket, ReceiptResponse};

/// The `PushNotifier` takes one or more `PushMessage` to send to the push notification server
///
/// ## Example:
///
/// ```
/// # use expo_server_sdk::{ExpoNotificationsClient, message::*};
/// # use std::str::FromStr;
/// # tokio_test::block_on(async {
///     let token = PushToken::from_str("ExpoPushToken[my-token]").unwrap();
///     let mut msg = PushMessage::new(token).body("test notification");
///
///     let client = ExpoNotificationsClient::new();
///     let result = client.send_push_notification(&msg).await;
/// # });
/// ```
///
pub struct ExpoNotificationsClient {
    pub push_url: Url,
    pub receipt_url: Url,
    pub authorization: Option<String>,
    pub gzip: GzipPolicy,
    pub push_chunk_size: usize,
    pub receipt_chunk_size: usize,
    client: reqwest::Client,
}

impl ExpoNotificationsClient {
    /// Create a new PushNotifier client.
    pub fn new() -> ExpoNotificationsClient {
        ExpoNotificationsClient {
            push_url: "https://exp.host/--/api/v2/push/send".parse().unwrap(),
            receipt_url: "https://exp.host/--/api/v2/push/getReceipts"
                .parse()
                .unwrap(),
            authorization: None,
            gzip: Default::default(),
            push_chunk_size: 100,
            receipt_chunk_size: 300,
            client: reqwest::Client::builder().gzip(true).build().unwrap(),
        }
    }

    /// Specify the URL to the push notification server push endpoint.
    /// Default is the Expo push notification server.
    pub fn push_url(mut self, url: Url) -> Self {
        self.push_url = url;
        self
    }

    /// Specify the URL to the push notification server getReceipts endpoint.
    /// Default is the Expo push notification server.
    pub fn receipt_url(mut self, url: Url) -> Self {
        self.receipt_url = url;
        self
    }

    /// Specify the authorization token (if enhanced push security is enabled).
    pub fn authorization(mut self, token: Option<String>) -> Self {
        self.authorization = token;
        self
    }

    /// Specify whether to compress the outgoing requests with gzip.
    pub fn gzip(mut self, gzip: GzipPolicy) -> Self {
        self.gzip = gzip;
        self
    }

    // Specify the chunk size to use for `send_push_notifications`. Should not be greater than 100 (the default).
    pub fn push_chunk_size(mut self, chunk_size: usize) -> Self {
        self.push_chunk_size = chunk_size;
        self
    }

    // Specify the chunk size to use for `get_push_receipts`. Should not be greater than 300 (the default).
    pub fn receipt_chunk_size(mut self, chunk_size: usize) -> Self {
        self.receipt_chunk_size = chunk_size;
        self
    }

    /// Sends a single [`PushMessage`] to the push notification server.
    pub async fn send_push_notification(
        &self,
        message: &PushMessage,
    ) -> Result<PushTicket, ExpoNotificationError> {
        let mut result = self
            .send_push_notifications_in_one_chunk(std::iter::once(message))
            .await?;
        Ok(result.pop().unwrap())
    }

    /// Sends an iterator of [`PushMessage`] to the server.
    /// This method automatically chunks the input message iterator.
    pub async fn send_push_notifications(
        &self,
        messages: impl IntoIterator<Item = impl Borrow<PushMessage>>,
    ) -> Result<Vec<PushTicket>, ExpoNotificationError> {
        let mut messages = messages.into_iter().peekable();
        let mut receipts = Vec::with_capacity(messages.size_hint().1.unwrap_or(0));
        while messages.peek().is_some() {
            let chunk_receipts = self
                .send_push_notifications_in_one_chunk(messages.by_ref().take(self.push_chunk_size))
                .await?;
            receipts.extend(chunk_receipts.into_iter());
        }
        Ok(receipts)
    }

    /// Send a single chunk of [`PushMessage`] to the server.
    ///
    /// If the provided messages chunk contains more than 100 items this might fail.
    /// Prefer the `send_push_notifications` in such situation.
    pub async fn send_push_notifications_in_one_chunk(
        &self,
        messages: impl IntoIterator<Item = impl Borrow<PushMessage>>,
    ) -> Result<Vec<PushTicket>, ExpoNotificationError> {
        let mut buffer = Vec::new();
        serialize_into_json_list(messages.into_iter(), &mut buffer)?;
        let res = self.send_request(self.push_url.clone(), buffer).await?;
        let res = res.json::<PushResponse>().await?;
        Ok(res.data)
    }

    /// Get a push notification receipt.
    pub async fn get_push_receipt(
        &self,
        receipt_id: &PushReceiptId,
    ) -> Result<Option<PushReceipt>, ExpoNotificationError> {
        let result = self
            .get_push_receipts_in_one_chunk(std::iter::once(receipt_id))
            .await?;
        Ok(result.into_values().next())
    }

    /// Get many push notification receipts.
    pub async fn get_push_receipts(
        &self,
        receipt_ids: impl IntoIterator<Item = impl Borrow<PushReceiptId>>,
    ) -> Result<HashMap<PushReceiptId, PushReceipt>, ExpoNotificationError> {
        let mut ids = receipt_ids.into_iter().peekable();
        let mut out = HashMap::new();
        while ids.peek().is_some() {
            let chunk_receipts = self
                .get_push_receipts_in_one_chunk(ids.by_ref().take(self.receipt_chunk_size))
                .await?;
            out.extend(chunk_receipts.into_iter());
        }
        Ok(out)
    }

    /// Get push notification receipts in one request. Avoid sending more than 300 receipt ids.
    pub async fn get_push_receipts_in_one_chunk(
        &self,
        receipt_ids: impl IntoIterator<Item = impl Borrow<PushReceiptId>>,
    ) -> Result<HashMap<PushReceiptId, PushReceipt>, ExpoNotificationError> {
        let mut buffer: Vec<u8> = "{\"ids\":".as_bytes().into();
        serialize_into_json_list(receipt_ids.into_iter(), &mut buffer)?;
        buffer.push('}' as u8);
        let res = self.send_request(self.receipt_url.clone(), buffer).await?;
        let res = res.json::<ReceiptResponse>().await?;
        Ok(res.data)
    }

    async fn send_request(
        &self,
        url: Url,
        buffer: Vec<u8>,
    ) -> Result<reqwest::Response, ExpoNotificationError> {
        let mut req = self
            .client
            .post(url)
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .header(ACCEPT_ENCODING, HeaderValue::from_static("gzip"))
            .header(ACCEPT_ENCODING, HeaderValue::from_static("deflate"))
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(auth_token) = self.authorization.as_ref() {
            req = req.bearer_auth(auth_token);
        }

        let should_compress = match self.gzip {
            GzipPolicy::ZipGreaterThanTreshold(treshold) if buffer.len() > treshold => true,
            GzipPolicy::Always => true,
            _ => false,
        };

        let body = if should_compress {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            use std::io::Write;

            req = req.header(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write(&buffer)?;
            encoder.finish()?
        } else {
            buffer
        };

        req = req.body(body);
        Ok(req.send().await?.error_for_status()?)
    }
}

fn serialize_into_json_list<T: Serialize>(
    mut data: impl Iterator<Item = impl Borrow<T>>,
    mut buffer: &mut Vec<u8>,
) -> Result<(), ExpoNotificationError> {
    buffer.push('[' as u8);
    let first_msg = data.next().ok_or(ExpoNotificationError::Empty)?;
    serde_json::to_writer(&mut buffer, first_msg.borrow()).unwrap();
    data.for_each(|msg| {
        buffer.push(',' as u8);
        serde_json::to_writer(&mut buffer, msg.borrow()).unwrap();
    });
    buffer.push(']' as u8);
    Ok(())
}
