/// How many requests can a receiver hold before it lags. See:
/// https://docs.rs/tokio/1.18.2/tokio/sync/broadcast/index.html#lagging
const BROADCAST_CHANNEL_CAPACITY: usize = 1000;

/// How many requests to keep per uuid. During insertions older requests are
/// removed to make space for new requests.
const MAX_REQUESTS_PER_UUID: usize = 1000;

/// When requests reach this age they will be removed during a cleanup.
const MAX_REQUEST_AGE_HOURS: i64 = 24;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Request {
    pub id: uuid::Uuid,
    pub received_time: chrono::DateTime<chrono::Utc>,
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub struct RequestRepository {
    map: std::collections::HashMap<uuid::Uuid, RequestsAndSender>,
}

struct RequestsAndSender {
    // Old requests at the front, new at the back.
    requests: std::collections::VecDeque<Request>,
    sender: tokio::sync::broadcast::Sender<Request>,
}

impl RequestRepository {
    pub fn new() -> Self {
        RequestRepository {
            map: std::collections::HashMap::new(),
        }
    }

    /// Inserts request into repository and sends it to the broadcast channel.
    /// Removes excess requests (exceeding MAX_REQUESTS_PER_UUID limit).
    pub fn insert(&mut self, uuid: uuid::Uuid, request: Request) {
        let requests_and_sender = self.map.entry(uuid).or_insert_with(|| RequestsAndSender {
            requests: std::collections::VecDeque::new(),
            sender: tokio::sync::broadcast::channel(BROADCAST_CHANNEL_CAPACITY).0,
        });
        while requests_and_sender.requests.len() >= MAX_REQUESTS_PER_UUID {
            requests_and_sender.requests.pop_front();
        }
        requests_and_sender.requests.push_back(request.clone());
        let _ = requests_and_sender.sender.send(request);
    }

    /// Returns a tuple with list of requests (from oldest to newest) and
    /// request receiver for given uuid.
    pub fn get_requests_and_receiver(
        &mut self,
        uuid: uuid::Uuid,
    ) -> (Vec<Request>, tokio::sync::broadcast::Receiver<Request>) {
        let requests_and_sender = self.map.entry(uuid).or_insert_with(|| RequestsAndSender {
            requests: std::collections::VecDeque::new(),
            sender: tokio::sync::broadcast::channel(BROADCAST_CHANNEL_CAPACITY).0,
        });
        (
            requests_and_sender.requests.iter().cloned().collect(),
            requests_and_sender.sender.subscribe(),
        )
    }

    /// Removes old requests (older than MAX_REQUEST_AGE_HOURS) and senders
    /// with no receivers.
    pub fn cleanup(&mut self) {
        let now = chrono::Utc::now();

        let mut tmp = std::collections::HashMap::new();
        std::mem::swap(&mut tmp, &mut self.map);

        self.map = tmp
            .into_iter()
            .map(|(uuid, mut requests_and_sender)| {
                let mut tmp = std::collections::VecDeque::new();
                std::mem::swap(&mut tmp, &mut requests_and_sender.requests);

                requests_and_sender.requests = tmp
                    .into_iter()
                    .filter(|request| {
                        let time_diff = now - request.received_time;
                        time_diff <= chrono::Duration::hours(MAX_REQUEST_AGE_HOURS)
                    })
                    .collect();

                (uuid, requests_and_sender)
            })
            .filter(|(_uuid, requests_and_sender)| {
                !(requests_and_sender.requests.is_empty()
                    && requests_and_sender.sender.receiver_count() == 0)
            })
            .collect();
    }
}
