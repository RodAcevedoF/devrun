use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

use crate::{Event, OutputSource, ResourceUsage, ServiceId};

const COALESCE_LIMIT: usize = 64 * 1024;

struct Pending {
    events: VecDeque<Event>,
    buffered: HashMap<ServiceId, usize>,
    dropped: BTreeMap<ServiceId, u64>,
    sampled: HashSet<ServiceId>,
    limit: usize,
    closed: bool,
}

impl Pending {
    fn push(&mut self, event: Event) {
        if self.closed {
            return;
        }
        match event {
            Event::Output {
                service,
                source,
                bytes,
            } => self.push_output(&service, source, bytes),
            Event::Usage { service, usage } => self.push_usage(service, usage),
            other => self.events.push_back(other),
        }
    }

    fn push_usage(&mut self, service: ServiceId, usage: ResourceUsage) {
        if self.sampled.contains(&service) {
            let pending = self.events.iter_mut().find_map(|event| match event {
                Event::Usage {
                    service: owner,
                    usage: pending,
                } if *owner == service => Some(pending),
                _ => None,
            });
            if let Some(pending) = pending {
                *pending = usage;
                return;
            }
        }
        self.sampled.insert(service.clone());
        self.events.push_back(Event::Usage { service, usage });
    }

    fn push_output(&mut self, service: &ServiceId, source: OutputSource, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        *self.buffered.entry(service.clone()).or_default() += bytes.len();
        match self.events.back_mut() {
            Some(Event::Output {
                service: last_service,
                source: last_source,
                bytes: last_bytes,
            }) if last_service == service
                && *last_source == source
                && last_bytes.len() + bytes.len() <= COALESCE_LIMIT =>
            {
                last_bytes.extend_from_slice(&bytes);
            }
            _ => self.events.push_back(Event::Output {
                service: service.clone(),
                source,
                bytes,
            }),
        }
        self.trim(service);
    }

    fn trim(&mut self, service: &ServiceId) {
        let buffered = self.buffered.get(service).copied().unwrap_or_default();
        let excess = buffered.saturating_sub(self.limit);
        if excess == 0 {
            return;
        }
        let removed = self.drop_oldest_output(service, excess);
        self.buffered.insert(service.clone(), buffered - removed);
        *self.dropped.entry(service.clone()).or_default() += removed as u64;
    }

    fn drop_oldest_output(&mut self, service: &ServiceId, excess: usize) -> usize {
        let mut removed = 0;
        let mut index = 0;
        while removed < excess && index < self.events.len() {
            if let Event::Output {
                service: owner,
                bytes,
                ..
            } = &mut self.events[index]
                && owner == service
            {
                let taken = (excess - removed).min(bytes.len());
                bytes.drain(..taken);
                removed += taken;
                if bytes.is_empty() {
                    self.events.remove(index);
                    continue;
                }
            }
            index += 1;
        }
        removed
    }

    fn pop(&mut self) -> Option<Event> {
        if let Some(notice) = self.drop_notice_before_front() {
            return Some(notice);
        }
        match self.events.pop_front() {
            Some(event) => {
                self.forget(&event);
                Some(event)
            }
            None => self.dropped.pop_first().map(drop_notice),
        }
    }

    fn forget(&mut self, event: &Event) {
        match event {
            Event::Output { service, bytes, .. } => {
                if let Some(buffered) = self.buffered.get_mut(service) {
                    *buffered = buffered.saturating_sub(bytes.len());
                }
            }
            Event::Usage { service, .. } => {
                self.sampled.remove(service);
            }
            _ => {}
        }
    }

    fn drop_notice_before_front(&mut self) -> Option<Event> {
        let Some(Event::Output { service, .. }) = self.events.front() else {
            return None;
        };
        self.dropped.remove_entry(service).map(drop_notice)
    }
}

fn drop_notice((service, bytes): (ServiceId, u64)) -> Event {
    Event::OutputDropped { service, bytes }
}

struct Shared {
    pending: Mutex<Pending>,
    ready: Notify,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, event: Event) {
        self.lock().push(event);
        self.ready.notify_one();
    }
}

pub(crate) fn event_queue(limit: usize) -> (Publisher, EventQueue) {
    let shared = Arc::new(Shared {
        pending: Mutex::new(Pending {
            events: VecDeque::new(),
            buffered: HashMap::new(),
            dropped: BTreeMap::new(),
            sampled: HashSet::new(),
            limit: limit.max(1),
            closed: false,
        }),
        ready: Notify::new(),
    });
    (
        Publisher {
            shared: Arc::clone(&shared),
        },
        EventQueue { shared },
    )
}

pub(crate) struct Publisher {
    shared: Arc<Shared>,
}

impl Publisher {
    pub fn publish(&self, event: Event) {
        self.shared.push(event);
    }

    pub fn output_sink(&self, service: ServiceId, source: OutputSource) -> OutputSink {
        OutputSink {
            shared: Arc::clone(&self.shared),
            service,
            source,
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.ready.notify_one();
    }
}

pub(crate) struct OutputSink {
    shared: Arc<Shared>,
    service: ServiceId,
    source: OutputSource,
}

impl OutputSink {
    pub fn send(&self, bytes: Vec<u8>) {
        self.shared.push(Event::Output {
            service: self.service.clone(),
            source: self.source,
            bytes,
        });
    }
}

pub(crate) struct EventQueue {
    shared: Arc<Shared>,
}

impl EventQueue {
    pub async fn recv(&self) -> Option<Event> {
        loop {
            {
                let mut pending = self.shared.lock();
                if let Some(event) = pending.pop() {
                    return Some(event);
                }
                if pending.closed {
                    return None;
                }
            }
            self.shared.ready.notified().await;
        }
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.shared.lock().pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(cpu_permille: u32) -> ResourceUsage {
        ResourceUsage {
            cpu_permille,
            rss_bytes: 1024,
            processes: 1,
        }
    }

    fn usage_event(service: &str, cpu_permille: u32) -> Event {
        Event::Usage {
            service: ServiceId::new(service),
            usage: usage(cpu_permille),
        }
    }

    fn rejected() -> Event {
        Event::Rejected {
            reason: "between".to_owned(),
        }
    }

    fn drain(queue: &EventQueue) -> Vec<Event> {
        std::iter::from_fn(|| queue.try_recv()).collect()
    }

    #[test]
    fn a_pending_usage_is_replaced_in_place_by_the_newest_sample() {
        let (publisher, queue) = event_queue(1024);

        publisher.publish(usage_event("api", 1));
        publisher.publish(rejected());
        publisher.publish(usage_event("web", 2));
        publisher.publish(usage_event("api", 3));

        assert_eq!(
            drain(&queue),
            vec![usage_event("api", 3), rejected(), usage_event("web", 2)]
        );
    }

    #[test]
    fn a_delivered_usage_no_longer_absorbs_later_samples() {
        let (publisher, queue) = event_queue(1024);

        publisher.publish(usage_event("api", 1));
        assert_eq!(drain(&queue), vec![usage_event("api", 1)]);
        publisher.publish(rejected());
        publisher.publish(usage_event("api", 2));

        assert_eq!(drain(&queue), vec![rejected(), usage_event("api", 2)]);
    }
}
