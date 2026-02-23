use super::IncomingMessage;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

/// Batches rapid-fire messages from the same session into a single message.
pub struct MessageCoalescer {
    debounce: Duration,
    input_rx: mpsc::UnboundedReceiver<IncomingMessage>,
    output_tx: mpsc::UnboundedSender<IncomingMessage>,
}

impl MessageCoalescer {
    pub fn new(
        debounce: Duration,
        input_rx: mpsc::UnboundedReceiver<IncomingMessage>,
        output_tx: mpsc::UnboundedSender<IncomingMessage>,
    ) -> Self {
        Self {
            debounce,
            input_rx,
            output_tx,
        }
    }

    /// Run the coalescer loop. Blocks until the input channel is closed.
    pub async fn run(mut self) {
        let mut pending: HashMap<String, Vec<IncomingMessage>> = HashMap::new();
        let mut deadlines: HashMap<String, Instant> = HashMap::new();

        loop {
            // Calculate next deadline
            let timeout = deadlines
                .values()
                .min()
                .map(|earliest| {
                    let now = Instant::now();
                    if *earliest > now {
                        *earliest - now
                    } else {
                        Duration::ZERO
                    }
                })
                .unwrap_or(Duration::from_secs(3600));

            tokio::select! {
                msg = self.input_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            let session = msg.session_id.clone();
                            pending.entry(session.clone()).or_default().push(msg);
                            deadlines.insert(session, Instant::now() + self.debounce);
                        }
                        None => {
                            // Channel closed — flush remaining
                            for (_session, messages) in pending.drain() {
                                let coalesced = coalesce_messages(messages);
                                let _ = self.output_tx.send(coalesced);
                            }
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(timeout) => {
                    let now = Instant::now();
                    let expired: Vec<String> = deadlines
                        .iter()
                        .filter(|(_, deadline)| **deadline <= now)
                        .map(|(k, _)| k.clone())
                        .collect();

                    for session in expired {
                        deadlines.remove(&session);
                        if let Some(messages) = pending.remove(&session) {
                            let coalesced = coalesce_messages(messages);
                            let _ = self.output_tx.send(coalesced);
                        }
                    }
                }
            }
        }
    }
}

/// Combine multiple messages into a single message with joined content.
fn coalesce_messages(mut messages: Vec<IncomingMessage>) -> IncomingMessage {
    if messages.len() == 1 {
        return messages.remove(0);
    }

    let first = &messages[0];
    let combined = messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    IncomingMessage {
        channel: first.channel.clone(),
        sender_id: first.sender_id.clone(),
        sender_name: first.sender_name.clone(),
        session_id: first.session_id.clone(),
        content: combined,
        reply_to: first.reply_to.clone(),
        timestamp: first.timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::now_ms;

    fn test_msg(session: &str, content: &str) -> IncomingMessage {
        IncomingMessage {
            channel: "test".into(),
            sender_id: "user1".into(),
            sender_name: Some("User".into()),
            session_id: session.into(),
            content: content.into(),
            reply_to: None,
            timestamp: now_ms(),
        }
    }

    #[tokio::test]
    async fn test_single_message_passthrough() {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (output_tx, mut output_rx) = mpsc::unbounded_channel();
        let coalescer = MessageCoalescer::new(Duration::from_millis(50), input_rx, output_tx);

        tokio::spawn(coalescer.run());

        input_tx.send(test_msg("s1", "hello")).unwrap();
        drop(input_tx); // close channel to trigger flush

        let msg = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(msg.content, "hello");
    }

    #[tokio::test]
    async fn test_coalesce_rapid_messages() {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (output_tx, mut output_rx) = mpsc::unbounded_channel();
        let coalescer = MessageCoalescer::new(Duration::from_millis(100), input_rx, output_tx);

        tokio::spawn(coalescer.run());

        // Send 3 messages rapidly (within debounce window)
        input_tx.send(test_msg("s1", "first")).unwrap();
        input_tx.send(test_msg("s1", "second")).unwrap();
        input_tx.send(test_msg("s1", "third")).unwrap();

        // Wait for debounce to fire
        let msg = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(msg.content, "first\nsecond\nthird");
        assert_eq!(msg.session_id, "s1");
    }

    #[tokio::test]
    async fn test_separate_sessions() {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (output_tx, mut output_rx) = mpsc::unbounded_channel();
        let coalescer = MessageCoalescer::new(Duration::from_millis(50), input_rx, output_tx);

        tokio::spawn(coalescer.run());

        input_tx.send(test_msg("s1", "hello s1")).unwrap();
        input_tx.send(test_msg("s2", "hello s2")).unwrap();

        // Should get 2 separate messages
        let msg1 = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let msg2 = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap();

        let sessions: Vec<String> = vec![msg1.session_id, msg2.session_id];
        assert!(sessions.contains(&"s1".to_string()));
        assert!(sessions.contains(&"s2".to_string()));
    }
}
