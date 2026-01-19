use eva_common::{EResult, Error};

pub trait Message {
    fn topic(&self) -> &str;
    fn data(&self) -> &[u8];
}

impl Message for psrt::Message {
    fn topic(&self) -> &str {
        psrt::Message::topic(self)
    }
    fn data(&self) -> &[u8] {
        psrt::Message::data(self)
    }
}

impl Message for paho_mqtt::message::Message {
    fn topic(&self) -> &str {
        paho_mqtt::message::Message::topic(self)
    }
    fn data(&self) -> &[u8] {
        paho_mqtt::message::Message::payload(self)
    }
}

#[async_trait::async_trait]
pub trait Client {
    async fn subscribe(&self, topics: &str, qos: i32) -> EResult<()>;
    async fn unsubscribe(&self, topic: &str) -> EResult<()>;
    async fn subscribe_bulk(&self, topics: &[&str], qos: i32) -> EResult<()>;
    async fn unsubscribe_bulk(&self, topics: &[&str]) -> EResult<()>;
    async fn publish(&self, topic: &str, data: Vec<u8>, qos: i32) -> EResult<()>;
    fn take_data_channel(
        &mut self,
        queue_size: usize,
    ) -> Option<async_channel::Receiver<Box<dyn Message + Send + Sync>>>;
    fn is_connected(&self) -> bool;
    async fn bye(&self) -> EResult<()>;
}

#[async_trait::async_trait]
impl Client for psrt::client::Client {
    #[inline]
    async fn subscribe(&self, topic: &str, _qos: i32) -> EResult<()> {
        psrt::client::Client::subscribe(self, topic.to_owned())
            .await
            .map_err(Error::io)
    }
    #[inline]
    async fn unsubscribe(&self, topic: &str) -> EResult<()> {
        psrt::client::Client::unsubscribe(self, topic.to_owned())
            .await
            .map_err(Error::io)
    }
    #[inline]
    async fn subscribe_bulk(&self, topics: &[&str], _qos: i32) -> EResult<()> {
        let data = topics.iter().map(|v| (*v).to_owned()).collect();
        psrt::client::Client::subscribe_bulk(self, data)
            .await
            .map_err(Error::io)
    }
    #[inline]
    async fn unsubscribe_bulk(&self, topics: &[&str]) -> EResult<()> {
        let data = topics.iter().map(|v| (*v).to_owned()).collect();
        psrt::client::Client::unsubscribe_bulk(self, data)
            .await
            .map_err(Error::io)
    }
    #[inline]
    async fn publish(&self, topic: &str, data: Vec<u8>, _qos: i32) -> EResult<()> {
        psrt::client::Client::publish(self, psrt::DEFAULT_PRIORITY, topic.to_owned(), data)
            .await
            .map_err(Error::io)
    }
    fn take_data_channel(
        &mut self,
        queue_size: usize,
    ) -> Option<async_channel::Receiver<Box<dyn Message + Send + Sync>>> {
        if let Some(dc) = psrt::client::Client::take_data_channel(self) {
            let (tx, rx) = async_channel::bounded::<Box<dyn Message + Send + Sync>>(queue_size);
            tokio::spawn(async move {
                while let Ok(message) = dc.recv().await {
                    if tx.send(Box::new(message)).await.is_err() {
                        break;
                    }
                }
            });
            Some(rx)
        } else {
            None
        }
    }
    #[inline]
    fn is_connected(&self) -> bool {
        psrt::client::Client::is_connected(self)
    }
    #[inline]
    async fn bye(&self) -> EResult<()> {
        psrt::client::Client::bye(self).await.map_err(Error::io)
    }
}

#[async_trait::async_trait]
impl Client for paho_mqtt::async_client::AsyncClient {
    #[inline]
    async fn subscribe(&self, topic: &str, qos: i32) -> EResult<()> {
        paho_mqtt::async_client::AsyncClient::subscribe(self, topic, qos)
            .await
            .map_err(Error::io)?;
        Ok(())
    }
    #[inline]
    async fn unsubscribe(&self, topic: &str) -> EResult<()> {
        paho_mqtt::async_client::AsyncClient::unsubscribe(self, topic)
            .await
            .map_err(Error::io)?;
        Ok(())
    }
    #[inline]
    async fn subscribe_bulk(&self, topics: &[&str], qos: i32) -> EResult<()> {
        paho_mqtt::async_client::AsyncClient::subscribe_many(
            self,
            topics,
            &vec![qos; topics.len()],
        )
        .await
        .map_err(Error::io)?;
        Ok(())
    }
    #[inline]
    async fn unsubscribe_bulk(&self, topics: &[&str]) -> EResult<()> {
        paho_mqtt::async_client::AsyncClient::unsubscribe_many(self, topics)
            .await
            .map_err(Error::io)?;
        Ok(())
    }
    #[inline]
    async fn publish(&self, topic: &str, data: Vec<u8>, qos: i32) -> EResult<()> {
        let message = paho_mqtt::Message::new(topic, data, qos);
        paho_mqtt::async_client::AsyncClient::publish(self, message)
            .await
            .map_err(Error::io)
    }
    fn take_data_channel(
        &mut self,
        queue_size: usize,
    ) -> Option<async_channel::Receiver<Box<dyn Message + Send + Sync>>> {
        let dc = paho_mqtt::async_client::AsyncClient::get_stream(self, queue_size);
        let (tx, rx) = async_channel::bounded::<Box<dyn Message + Send + Sync>>(queue_size);
        tokio::spawn(async move {
            while let Ok(message) = dc.recv().await {
                if let Some(msg) = message
                    && tx.send(Box::new(msg)).await.is_err()
                {
                    break;
                }
            }
        });
        Some(rx)
    }
    #[inline]
    fn is_connected(&self) -> bool {
        paho_mqtt::async_client::AsyncClient::is_connected(self)
    }
    #[inline]
    async fn bye(&self) -> EResult<()> {
        paho_mqtt::async_client::AsyncClient::disconnect(
            self,
            paho_mqtt::disconnect_options::DisconnectOptions::default(),
        )
        .await
        .map_err(Error::io)?;
        Ok(())
    }
}
