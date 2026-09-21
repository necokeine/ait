use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use server_ports::registry::{MutationListener, MutationSubscription, RegistryError};

type ListenerList<T> = Mutex<Vec<Weak<Registration<T>>>>;

struct Registration<T>(MutationListener<T>);

pub(super) struct Listeners<T>(ListenerList<T>);

impl<T> Default for Listeners<T> {
    fn default() -> Self {
        Self(Mutex::new(Vec::new()))
    }
}

impl<T> fmt::Debug for Listeners<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Listeners { .. }")
    }
}

struct Subscription<T>(Arc<Registration<T>>);

impl<T> fmt::Debug for Subscription<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("owners", &Arc::strong_count(&self.0))
            .finish()
    }
}
impl<T> MutationSubscription for Subscription<T> {}

impl<T: 'static> Listeners<T> {
    pub(super) fn subscribe(&self, listener: MutationListener<T>) -> Box<dyn MutationSubscription> {
        let mut listeners = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        listeners.retain(|listener| listener.strong_count() != 0);
        let registration = Arc::new(Registration(listener));
        listeners.push(Arc::downgrade(&registration));
        Box::new(Subscription(registration))
    }

    pub(super) fn notify(&self, mutation: &T, suppress_errors: bool) -> Result<(), RegistryError> {
        let listeners: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        let mut failed = false;
        for listener in listeners {
            if (listener.0)(mutation).is_err() {
                failed = true;
                tracing::error!("registry mutation observer failed after commit");
            }
        }
        if failed && !suppress_errors {
            Err(RegistryError::Observer)
        } else {
            Ok(())
        }
    }
}
