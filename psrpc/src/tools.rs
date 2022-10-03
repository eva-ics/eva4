use crate::{pubsub::Message, Error};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

pub type PublicationSender = async_channel::Sender<Publication>;
pub type PublicationReceiver = async_channel::Receiver<Publication>;

pub struct Publication {
    subtopic_pos: usize,
    pub message: Box<dyn Message + Send + Sync + 'static>,
}

impl Publication {
    #[inline]
    pub fn message(&self) -> &(dyn Message + 'static) {
        self.message.as_ref()
    }
    #[inline]
    pub fn topic(&self) -> &str {
        self.message.topic()
    }
    /// # Panics
    ///
    /// Will not panic as all processed frames always have topics
    #[inline]
    pub fn subtopic(&self) -> &str {
        &self.message.topic()[self.subtopic_pos..]
    }
    #[inline]
    pub fn data(&self) -> &[u8] {
        self.message.data()
    }
}

/// Topic publications broker
///
/// The helper class to process topics in blocking mode
///
/// Processes topics and sends frames to handler channels
#[derive(Default)]
pub struct TopicBroker {
    prefixes: BTreeMap<String, PublicationSender>,
    topics: BTreeMap<String, PublicationSender>,
}

impl TopicBroker {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    /// Process a topic (returns tx, rx channel for a handler)
    #[inline]
    pub fn register_topic(
        &mut self,
        topic: &str,
        channel_size: usize,
    ) -> Result<(PublicationSender, PublicationReceiver), Error> {
        let (tx, rx) = async_channel::bounded(channel_size);
        self.register_topic_tx(topic, tx.clone())?;
        Ok((tx, rx))
    }
    /// Process a topic with the pre-defined channel
    #[inline]
    pub fn register_topic_tx(&mut self, topic: &str, tx: PublicationSender) -> Result<(), Error> {
        if let Entry::Vacant(o) = self.topics.entry(topic.to_owned()) {
            o.insert(tx);
            Ok(())
        } else {
            Err(Error::busy("topic already registered"))
        }
    }
    /// Process subtopic by prefix (returns tx, rx channel for a handler)
    #[inline]
    pub fn register_prefix(
        &mut self,
        prefix: &str,
        channel_size: usize,
    ) -> Result<(PublicationSender, PublicationReceiver), Error> {
        let (tx, rx) = async_channel::bounded(channel_size);
        self.register_prefix_tx(prefix, tx.clone())?;
        Ok((tx, rx))
    }
    /// Process subtopic by prefix with the pre-defined channel
    #[inline]
    pub fn register_prefix_tx(&mut self, prefix: &str, tx: PublicationSender) -> Result<(), Error> {
        if let Entry::Vacant(o) = self.prefixes.entry(prefix.to_owned()) {
            o.insert(tx);
            Ok(())
        } else {
            Err(Error::busy("topic prefix already registered"))
        }
    }
    /// The message is returned back if not processed
    #[inline]
    pub async fn process(
        &self,
        message: Box<dyn Message + Sync + Send + 'static>,
    ) -> Result<Option<Box<dyn Message + Send + Sync + 'static>>, Error> {
        let topic = message.topic();
        if let Some(tx) = self.topics.get(topic) {
            tx.send(Publication {
                subtopic_pos: 0,
                message,
            })
            .await
            .map_err(Error::core)?;
            return Ok(None);
        }
        for (pfx, tx) in &self.prefixes {
            if topic.starts_with(pfx) {
                tx.send(Publication {
                    subtopic_pos: pfx.len(),
                    message,
                })
                .await
                .map_err(Error::io)?;
                return Ok(None);
            }
        }
        Ok(Some(message))
    }
}
