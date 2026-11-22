//! The connected providers, and which one owns each box.
//!
//! The shell holds one registry. Every call about a box goes through
//! [`Registry::owner`], which answers from what the last listing showed
//! and, for a box it has not seen, asks every provider at once. A box is
//! only ever routed to a provider that said it has it.
//!
//! Ids can collide: Vercel, Blaxel and Sprites use the name the person
//! chose, so `api` can exist in two of them. Calls take only the id, so an
//! id that more than one provider claims is refused rather than guessed,
//! until one of the boxes is renamed or deleted.

use crate::provider::Provider;
use crate::types::Machine;
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Default)]
pub struct Registry {
    providers: Mutex<Vec<Arc<dyn Provider>>>,
    /// Every provider that has listed or returned a box with this id.
    owners: Mutex<HashMap<String, Vec<&'static str>>>,
}

/// The data stays consistent if a holder panicked: every critical section
/// is a single insert, remove or clone.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn ambiguous(machine_id: &str, owners: &[&'static str]) -> Error {
    Error::Provider(format!(
        "{} each have a box called {machine_id}, so Stowbox cannot tell which one is meant. Rename or delete one of them.",
        owners.join(" and ")
    ))
}

/// What one listing across every provider returned: the boxes of those that
/// answered, and the error of each one that did not.
#[derive(Default)]
pub struct Listing {
    pub machines: Vec<Machine>,
    pub errors: Vec<(&'static str, Error)>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `p`, replacing a provider with the same id. Providers keep the
    /// order they were first added in.
    pub fn set(&self, p: Arc<dyn Provider>) {
        let mut all = lock(&self.providers);
        match all.iter_mut().find(|q| q.id() == p.id()) {
            Some(slot) => *slot = p,
            None => all.push(p),
        }
    }

    /// Drops the provider and forgets which boxes it owned.
    pub fn remove(&self, id: &str) {
        lock(&self.providers).retain(|p| p.id() != id);
        lock(&self.owners).retain(|_, owners| {
            owners.retain(|o| *o != id);
            !owners.is_empty()
        });
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        lock(&self.providers).iter().find(|p| p.id() == id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn Provider>> {
        lock(&self.providers).clone()
    }

    pub fn is_empty(&self) -> bool {
        lock(&self.providers).is_empty()
    }

    /// Records that `m` belongs to `provider`, and stamps it on the machine.
    fn claim(&self, provider: &'static str, m: &mut Machine) {
        m.provider = provider.to_string();
        let mut owners = lock(&self.owners);
        let claims = owners.entry(m.id.clone()).or_default();
        if !claims.contains(&provider) {
            claims.push(provider);
        }
    }

    /// Drops `provider`'s claim to every box it did not just list.
    fn keep_only(&self, provider: &'static str, listed: &HashSet<&str>) {
        lock(&self.owners).retain(|id, owners| {
            if !listed.contains(id.as_str()) {
                owners.retain(|o| *o != provider);
            }
            !owners.is_empty()
        });
    }

    pub fn forget(&self, machine_id: &str) {
        lock(&self.owners).remove(machine_id);
    }

    /// Every provider's boxes, listed at the same time. One provider that
    /// fails does not hide the others' boxes.
    pub async fn list_machines(&self) -> Listing {
        let providers = self.all();
        let results = futures_join_all(providers.iter().map(|p| {
            let p = p.clone();
            async move { (p.id(), p.list_machines().await) }
        }))
        .await;
        let mut out = Listing::default();
        for (id, r) in results {
            match r {
                Ok(ms) => {
                    self.keep_only(id, &ms.iter().map(|m| m.id.as_str()).collect());
                    for mut m in ms {
                        self.claim(id, &mut m);
                        out.machines.push(m);
                    }
                }
                Err(e) => out.errors.push((id, e)),
            }
        }
        out
    }

    /// The provider that owns `machine_id`.
    pub async fn owner(&self, machine_id: &str) -> Result<Arc<dyn Provider>> {
        let providers = self.all();
        if providers.is_empty() {
            return Err(Error::NotConfigured);
        }
        let known = lock(&self.owners).get(machine_id).cloned().unwrap_or_default();
        let connected: Vec<_> = providers.iter().filter(|p| known.contains(&p.id())).cloned().collect();
        match connected.as_slice() {
            [one] => return Ok(one.clone()),
            [] => {}
            _ => return Err(ambiguous(machine_id, &known)),
        }
        if let [only] = providers.as_slice() {
            return Ok(only.clone());
        }
        // Not seen yet: ask every provider at once, and take the one that
        // has it, unless several do.
        let answers = futures_join_all(providers.iter().map(|p| async move { (p.clone(), p.get_machine(machine_id).await) })).await;
        let mut found = Vec::new();
        let mut last = None;
        for (p, r) in answers {
            match r {
                Ok(mut m) => {
                    self.claim(p.id(), &mut m);
                    found.push(p);
                }
                Err(e) => last = Some(e),
            }
        }
        match found.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(match last {
                Some(Error::NotFound(_)) | None => Error::NotFound(format!("no connected provider has the box {machine_id}")),
                Some(e) => e,
            }),
            _ => Err(ambiguous(machine_id, &found.iter().map(|p| p.id()).collect::<Vec<_>>())),
        }
    }

    /// Reads one box from its owner, with the owner stamped on it.
    pub async fn get_machine(&self, machine_id: &str) -> Result<(Arc<dyn Provider>, Machine)> {
        let p = self.owner(machine_id).await?;
        let mut m = p.get_machine(machine_id).await?;
        self.claim(p.id(), &mut m);
        Ok((p, m))
    }

    /// The provider a new box goes to: the one named, or the only one.
    pub fn for_create(&self, provider: Option<&str>) -> Result<Arc<dyn Provider>> {
        let providers = self.all();
        match provider.filter(|p| !p.is_empty()) {
            Some(id) => providers.into_iter().find(|p| p.id() == id).ok_or_else(|| Error::Provider(format!("{id} is not connected"))),
            None => match providers.as_slice() {
                [] => Err(Error::NotConfigured),
                [only] => Ok(only.clone()),
                _ => Err(Error::Provider("more than one provider is connected: say which one makes the box".into())),
            },
        }
    }

    /// Stamps the owner on a machine a provider just made.
    pub fn adopt(&self, p: &Arc<dyn Provider>, mut m: Machine) -> Machine {
        self.claim(p.id(), &mut m);
        m
    }
}

/// Runs the futures concurrently and returns their outputs in order.
async fn futures_join_all<F: Future>(futs: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    // `join_all` lives in the `futures` crate, which this crate does not
    // pull for one call: poll them together here.
    let mut pinned: Vec<std::pin::Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut results: Vec<Option<F::Output>> = (0..pinned.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (f, slot) in pinned.iter_mut().zip(results.iter_mut()) {
            if slot.is_some() {
                continue;
            }
            match f.as_mut().poll(cx) {
                std::task::Poll::Ready(v) => *slot = Some(v),
                std::task::Poll::Pending => pending = true,
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    // poll_fn returned Ready only once every slot was filled.
    results.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::Capabilities;
    use crate::cost::RateCard;
    use crate::testing::machine;
    use crate::types::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        id: &'static str,
        boxes: Vec<&'static str>,
        fail_list: bool,
        gets: AtomicUsize,
    }

    fn fake(id: &'static str, boxes: &[&'static str]) -> Arc<Fake> {
        Arc::new(Fake { id, boxes: boxes.to_vec(), fail_list: false, gets: AtomicUsize::new(0) })
    }

    #[async_trait::async_trait]
    impl Provider for Fake {
        fn id(&self) -> &'static str {
            self.id
        }
        fn capabilities(&self) -> Capabilities {
            crate::boxd::capabilities()
        }
        async fn whoami(&self) -> Result<Account> {
            unimplemented!()
        }
        async fn list_machines(&self) -> Result<Vec<Machine>> {
            if self.fail_list {
                return Err(Error::Network("down".into()));
            }
            Ok(self.boxes.iter().map(|b| machine(b, MachineStatus::Running)).collect())
        }
        async fn get_machine(&self, id: &str) -> Result<Machine> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            if self.boxes.contains(&id) {
                Ok(machine(id, MachineStatus::Running))
            } else {
                Err(Error::NotFound(id.into()))
            }
        }
        async fn create_machine(&self, _: CreateSpec) -> Result<Machine> {
            unimplemented!()
        }
        async fn fork_machine(&self, _: &str, _: Option<String>) -> Result<Machine> {
            unimplemented!()
        }
        async fn power(&self, _: &str, _: PowerAction) -> Result<()> {
            unimplemented!()
        }
        async fn set_auto_hibernate(&self, _: &str, _: u32) -> Result<()> {
            unimplemented!()
        }
        async fn set_auto_suspend(&self, _: &str, _: u32) -> Result<()> {
            unimplemented!()
        }
        async fn exec(&self, _: &str, _: ExecSpec) -> Result<ExecHandle> {
            unimplemented!()
        }
        async fn machine_usage(&self, _: &str) -> Result<Usage> {
            unimplemented!()
        }
        async fn org_billing(&self, _: &str) -> Result<OrgBilling> {
            unimplemented!()
        }
        async fn rates(&self) -> Result<RateCard> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn listing_stamps_owners_and_keeps_the_boxes_of_providers_that_answer() {
        let r = Registry::new();
        r.set(fake("boxd", &["vm_1"]));
        r.set(Arc::new(Fake { id: "boat", boxes: vec!["bx_1"], fail_list: true, gets: AtomicUsize::new(0) }));
        let l = r.list_machines().await;
        assert_eq!(l.machines.iter().map(|m| (m.id.as_str(), m.provider.as_str())).collect::<Vec<_>>(), vec![("vm_1", "boxd")]);
        assert_eq!(l.errors.len(), 1);
        assert_eq!(l.errors[0].0, "boat");
    }

    #[tokio::test]
    async fn owner_answers_from_the_listing_then_by_asking() {
        let boxd = fake("boxd", &["vm_1"]);
        let boat = fake("boat", &["bx_1"]);
        let r = Registry::new();
        r.set(boxd.clone());
        r.set(boat.clone());
        r.list_machines().await;
        assert_eq!(r.owner("bx_1").await.unwrap().id(), "boat");
        assert_eq!(r.owner("vm_1").await.unwrap().id(), "boxd");
        assert_eq!(boxd.gets.load(Ordering::SeqCst) + boat.gets.load(Ordering::SeqCst), 0, "known boxes need no lookup");

        // A box made elsewhere since the listing: asked for, then remembered.
        let r = Registry::new();
        r.set(boxd.clone());
        r.set(boat.clone());
        assert_eq!(r.owner("bx_1").await.unwrap().id(), "boat");
        let asked = boat.gets.load(Ordering::SeqCst);
        assert_eq!(r.owner("bx_1").await.unwrap().id(), "boat");
        assert_eq!(boat.gets.load(Ordering::SeqCst), asked);

        assert!(matches!(r.owner("nope").await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn an_id_two_providers_have_is_refused_not_guessed() {
        let vercel = fake("vercel", &["api", "web"]);
        let blaxel = fake("blaxel", &["api"]);
        let r = Registry::new();
        r.set(vercel.clone());
        r.set(blaxel.clone());
        // Not listed yet: both are asked, and both have it.
        let e = r.owner("api").await.err().unwrap();
        assert!(e.to_string().contains("vercel and blaxel"), "{e}");
        // Listed: still both.
        r.list_machines().await;
        assert!(r.owner("api").await.is_err());
        assert_eq!(r.owner("web").await.unwrap().id(), "vercel");
        // Once one provider stops listing it, the other owns it again.
        let r2 = Registry::new();
        r2.set(fake("vercel", &["web"]));
        r2.set(blaxel.clone());
        r2.claim("vercel", &mut machine("api", MachineStatus::Running));
        r2.list_machines().await;
        assert_eq!(r2.owner("api").await.unwrap().id(), "blaxel");
    }

    #[tokio::test]
    async fn removing_a_provider_forgets_its_boxes() {
        let r = Registry::new();
        r.set(fake("boxd", &["vm_1"]));
        r.set(fake("boat", &["bx_1"]));
        r.list_machines().await;
        r.remove("boat");
        // The one provider left is asked directly; it does not own bx_1.
        assert_eq!(r.owner("bx_1").await.unwrap().id(), "boxd");
        assert!(r.get("boat").is_none());
        r.remove("boxd");
        assert!(matches!(r.owner("vm_1").await, Err(Error::NotConfigured)));
    }

    #[test]
    fn create_needs_a_named_provider_when_two_are_connected() {
        let r = Registry::new();
        assert!(matches!(r.for_create(None), Err(Error::NotConfigured)));
        r.set(fake("boxd", &[]));
        assert_eq!(r.for_create(None).unwrap().id(), "boxd");
        r.set(fake("boat", &[]));
        assert!(r.for_create(None).is_err());
        assert_eq!(r.for_create(Some("boat")).unwrap().id(), "boat");
        assert!(r.for_create(Some("aws")).is_err());
        // Replacing keeps the order.
        r.set(fake("boxd", &["vm_9"]));
        assert_eq!(r.all().iter().map(|p| p.id()).collect::<Vec<_>>(), vec!["boxd", "boat"]);
    }
}
