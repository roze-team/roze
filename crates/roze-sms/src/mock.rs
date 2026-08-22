use crate::{SmsError, SmsMessage, SmsProvider, SmsSendResult};
use async_trait::async_trait;
use std::{collections::VecDeque, fmt, sync::Mutex};

#[derive(Debug, Clone)]
pub enum MockSmsResponse {
    Success(SmsSendResult),
    Failure(SmsError),
}

/// Deterministic FIFO provider for application tests. Captured messages are
/// intentionally unavailable through `Debug` to prevent accidental disclosure.
pub struct MockSmsProvider {
    responses: Mutex<VecDeque<MockSmsResponse>>,
    sent: Mutex<Vec<SmsMessage>>,
}

impl MockSmsProvider {
    pub fn new(responses: impl IntoIterator<Item = MockSmsResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            sent: Mutex::new(Vec::new()),
        }
    }

    pub fn succeeding() -> Self {
        Self::new([MockSmsResponse::Success(SmsSendResult {
            provider: "mock".to_string(),
            provider_code: "OK".to_string(),
            provider_message: Some("OK".to_string()),
            biz_id: Some("mock-biz-1".to_string()),
            request_id: Some("mock-request-1".to_string()),
        })])
    }

    pub fn take_sent(&self) -> Vec<SmsMessage> {
        std::mem::take(&mut *self.sent.lock().expect("mock SMS sent lock poisoned"))
    }
}

impl fmt::Debug for MockSmsProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MockSmsProvider")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl SmsProvider for MockSmsProvider {
    async fn send(&self, message: SmsMessage) -> Result<SmsSendResult, SmsError> {
        self.sent
            .lock()
            .expect("mock SMS sent lock poisoned")
            .push(message);
        match self
            .responses
            .lock()
            .expect("mock SMS response lock poisoned")
            .pop_front()
        {
            Some(MockSmsResponse::Success(result)) => Ok(result),
            Some(MockSmsResponse::Failure(error)) => Err(error),
            None => panic!("mock SMS response queue exhausted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deterministic_provider_captures_messages_in_order() {
        let provider = MockSmsProvider::succeeding();
        provider
            .send(SmsMessage::new(["13900000000"]))
            .await
            .unwrap();
        let sent = provider.take_sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].phone_numbers, ["13900000000"]);
        assert!(!format!("{provider:?}").contains("13900000000"));
    }
}
