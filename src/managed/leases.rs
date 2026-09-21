use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LeaseError {
    Busy,
    NotOwner,
    Invalid,
    Expired,
    Entropy,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LeaseGrant {
    pub owner: String,
    pub generation: u64,
}

struct Lease {
    id: String,
    grant: LeaseGrant,
    deadline: Instant,
    attached: bool,
}
#[derive(Default)]
struct RegistryState {
    owner: Option<String>,
    lease: Option<Lease>,
}

pub struct LeaseRegistry {
    instance_id: String,
    state: Mutex<RegistryState>,
}

impl LeaseRegistry {
    #[must_use]
    pub fn new(instance_id: String) -> Self {
        Self {
            instance_id,
            state: Mutex::new(RegistryState::default()),
        }
    }

    pub fn register_browser(&self, owner: &str) -> Result<(), LeaseError> {
        let mut state = self.state.lock().unwrap();
        if state
            .owner
            .as_deref()
            .is_some_and(|current| current != owner)
        {
            return Err(LeaseError::Busy);
        }
        state.owner = Some(owner.into());
        Ok(())
    }

    pub fn unregister_browser(&self, owner: &str) {
        let mut state = self.state.lock().unwrap();
        if state.owner.as_deref() == Some(owner) {
            state.owner = None;
            state.lease = None;
        }
    }

    pub fn issue(&self, owner: &str, generation: u64, now: Instant) -> Result<String, LeaseError> {
        let mut state = self.state.lock().unwrap();
        if state.owner.as_deref() != Some(owner) {
            return Err(LeaseError::NotOwner);
        }
        if generation == 0 || generation > super::protocol::MAX_COUNTER {
            return Err(LeaseError::Invalid);
        }
        if state
            .lease
            .as_ref()
            .is_some_and(|lease| lease.attached || now < lease.deadline)
        {
            return Err(LeaseError::Busy);
        }
        let id = random_id().map_err(|_| LeaseError::Entropy)?;
        state.lease = Some(Lease {
            id: id.clone(),
            grant: LeaseGrant {
                owner: owner.into(),
                generation,
            },
            deadline: now + Duration::from_secs(30),
            attached: false,
        });
        Ok(id)
    }

    pub fn attach(
        &self,
        instance_id: &str,
        lease_id: &str,
        now: Instant,
    ) -> Result<LeaseGrant, LeaseError> {
        if instance_id != self.instance_id {
            return Err(LeaseError::Invalid);
        }
        let mut state = self.state.lock().unwrap();
        let lease = state
            .lease
            .as_mut()
            .filter(|lease| lease.id == lease_id)
            .ok_or(LeaseError::Invalid)?;
        if lease.attached {
            return Err(LeaseError::Busy);
        }
        if now >= lease.deadline {
            return Err(LeaseError::Expired);
        }
        lease.attached = true;
        Ok(lease.grant.clone())
    }

    pub fn release(&self, lease_id: &str) {
        let mut state = self.state.lock().unwrap();
        if state
            .lease
            .as_ref()
            .is_some_and(|lease| lease.id == lease_id)
        {
            state.lease = None;
        }
    }

    #[must_use]
    pub fn is_active(&self, lease_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.id == lease_id && lease.attached)
    }
}

pub fn random_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut result = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            result.push('-');
        }
        use std::fmt::Write;
        let _ = write!(result, "{byte:02x}");
    }
    Ok(result)
}
