use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};

use rosc::OscError;
use rosc::address::{Matcher, OscAddress};

use crate::{Action, OscEvent, Responder};

/// An OSC address pattern (`*`, `?`, `[..]`, `{a,b}` wildcards) and its actions.
pub struct Route {
    pattern: String,
    matcher: Matcher,
    actions: Vec<Action>,
}

impl Route {
    pub fn new(pattern: &str, actions: Vec<Action>) -> Result<Self, OscError> {
        Ok(Self { pattern: pattern.to_owned(), matcher: Matcher::new(pattern)?, actions })
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    fn matches(&self, address: &OscAddress) -> bool {
        self.matcher.match_address(address)
    }
}

/// Matches events against routes and dispatches the actions to responders by kind.
#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
    responders: HashMap<String, Box<dyn Responder>>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_route(&mut self, pattern: &str, actions: Vec<Action>) -> Result<(), OscError> {
        self.routes.push(Route::new(pattern, actions)?);
        Ok(())
    }

    /// Registers `responder` for actions of `kind` (`"sound"`, `"wled"`, or a custom kind).
    /// Replaces any responder previously registered for that kind.
    pub fn register(&mut self, kind: impl Into<String>, responder: impl Responder) {
        self.responders.insert(kind.into(), Box::new(responder));
    }

    /// Dispatches every action of every matching route. Returns how many actions were handled
    /// successfully; failures are logged and do not stop other actions.
    pub fn dispatch(&mut self, event: &OscEvent) -> usize {
        let Ok(address) = OscAddress::new(event.address.clone()) else {
            log::warn!("ignoring invalid OSC address {:?}", event.address);
            return 0;
        };
        let mut handled = 0;
        let mut matched = false;
        for route in self.routes.iter().filter(|r| r.matches(&address)) {
            matched = true;
            for action in &route.actions {
                let Some(responder) = self.responders.get_mut(action.kind()) else {
                    log::warn!("no responder for {:?} (route {})", action.kind(), route.pattern);
                    continue;
                };
                match responder.handle(action, event) {
                    Ok(()) => handled += 1,
                    Err(e) => log::warn!("{} failed on {}: {e}", responder.name(), event.address),
                }
            }
        }
        if !matched {
            log::debug!("no route for {}", event.address);
        }
        handled
    }

    /// Dispatches events from `events` on a dedicated thread until the sender is dropped.
    pub fn spawn(mut self, events: Receiver<OscEvent>) -> JoinHandle<()> {
        thread::Builder::new()
            .name("scareflow-osc-router".into())
            .spawn(move || {
                for event in events {
                    self.dispatch(&event);
                }
            })
            .expect("spawn router thread")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::{ChannelResponder, ResponderError, SoundAction};

    struct Failing;
    impl Responder for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        fn handle(&mut self, _: &Action, _: &OscEvent) -> Result<(), ResponderError> {
            Err("boom".into())
        }
    }

    fn custom(kind: &str) -> Action {
        Action::Custom { kind: kind.into(), data: serde_json::Value::Null }
    }

    #[test]
    fn wildcard_routes_dispatch_all_actions() {
        let (tx, rx) = mpsc::channel();
        let mut router = Router::new();
        router.register("sound", ChannelResponder::new("sound", tx.clone()));
        router.register("app", ChannelResponder::new("app", tx));
        router
            .add_route(
                "/pumpkin/*/scare",
                vec![Action::Sound(SoundAction::StopEffects), custom("app")],
            )
            .unwrap();
        router.add_route("/pumpkin/{left,right}/glow", vec![custom("app")]).unwrap();

        assert_eq!(router.dispatch(&OscEvent::new("/pumpkin/porch/scare", vec![])), 2);
        assert_eq!(router.dispatch(&OscEvent::new("/pumpkin/left/glow", vec![])), 1);
        assert_eq!(router.dispatch(&OscEvent::new("/pumpkin/middle/glow", vec![])), 0);
        assert_eq!(router.dispatch(&OscEvent::new("/pumpkin/a/b/scare", vec![])), 0);
        let kinds: Vec<_> = rx.try_iter().map(|(a, _)| a.kind().to_owned()).collect();
        assert_eq!(kinds, ["sound", "app", "app"]);
    }

    #[test]
    fn failing_or_missing_responder_does_not_block_others() {
        let (tx, rx) = mpsc::channel();
        let mut router = Router::new();
        router.register("bad", Failing);
        router.register("app", ChannelResponder::new("app", tx));
        router.add_route("/x", vec![custom("bad"), custom("missing"), custom("app")]).unwrap();
        assert_eq!(router.dispatch(&OscEvent::new("/x", vec![])), 1);
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn rejects_invalid_pattern() {
        assert!(Router::new().add_route("no-slash", vec![]).is_err());
    }
}
