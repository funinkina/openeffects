//! The set of lazily-loaded model sessions the effects run on. A provider swap
//! builds a whole new [`Engine`] off the frame path and replaces the old one.

use std::time::{Duration, Instant};

use tracing::warn;

use super::hand::{HandLandmarker, PalmDetector};
use super::models::{AutoLight, SelfieSeg, YuNet};

/// A lazily-loaded model, with a one-shot failure memo so a missing model
/// is logged once rather than retried every frame, and a last-use stamp so
/// an idle model can be dropped and reloaded on demand.
struct Slot<T> {
    model: Option<T>,
    active_ep: Option<&'static str>,
    failed: bool,
    used: Instant,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            model: None,
            active_ep: None,
            failed: false,
            used: Instant::now(),
        }
    }
}

impl<T> Slot<T> {
    fn ensure_loaded(
        &mut self,
        provider: &str,
        load: impl FnOnce(&str) -> anyhow::Result<(T, &'static str, Option<String>)>,
        unavailable: &str,
        fallback_out: &mut Option<String>,
    ) {
        if self.model.is_none() && !self.failed {
            match load(provider) {
                Ok((m, ep, fallback)) => {
                    self.model = Some(m);
                    self.active_ep = Some(ep);
                    if let Some(fb) = fallback {
                        *fallback_out = Some(fb);
                    }
                }
                Err(err) => {
                    warn!(%err, "{unavailable}");
                    self.failed = true;
                }
            }
        }
    }

    fn as_mut(&mut self) -> Option<&mut T> {
        self.used = Instant::now();
        self.model.as_mut()
    }

    #[cfg(test)]
    fn get_simple(
        &mut self,
        load: impl FnOnce() -> anyhow::Result<T>,
        unavailable: &str,
    ) -> Option<&mut T> {
        let mut fb = None;
        self.ensure_loaded(
            "cpu",
            |_| load().map(|m| (m, "cpu", None)),
            unavailable,
            &mut fb,
        );
        self.as_mut()
    }

    fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    /// A slot holding a model already loaded for a new provider, or an empty
    /// one that loads lazily on first use.
    fn preloaded(loaded: Option<(T, &'static str)>) -> Self {
        let mut slot = Self::default();
        if let Some((model, ep)) = loaded {
            slot.model = Some(model);
            slot.active_ep = Some(ep);
        }
        slot
    }

    fn unload_idle(&mut self, max_idle: Duration) -> bool {
        if self.model.is_some() && self.used.elapsed() >= max_idle {
            self.model = None;
            self.active_ep = None;
            return true;
        }
        false
    }
}

/// Which models an [`Engine`] has loaded, so a replacement loads the same set.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadedModels {
    selfie: bool,
    yunet: bool,
    palm: bool,
    hand: bool,
    auto_light: bool,
}

pub struct Engine {
    pub generation: u64,
    pub provider: String,
    pub active: &'static str,
    pub detail: Option<String>,
    selfie: Slot<SelfieSeg>,
    yunet: Slot<YuNet>,
    palm: Slot<PalmDetector>,
    hand: Slot<HandLandmarker>,
    auto_light: Slot<AutoLight>,
}

impl Default for Engine {
    fn default() -> Self {
        let requested = super::requested_ep();
        let mut eng = Self {
            generation: 0,
            provider: requested,
            active: "cpu",
            detail: None,
            selfie: Slot::default(),
            yunet: Slot::default(),
            palm: Slot::default(),
            hand: Slot::default(),
            auto_light: Slot::default(),
        };
        eng.update_active_status();
        eng
    }
}

impl Engine {
    /// Load `wanted` on `provider` into a fresh engine. Slow (each model
    /// compiles for the provider), so callers run it without holding the
    /// frame lock and then [`replace`](Self::replace) the live engine.
    pub fn load(provider: &str, wanted: LoadedModels) -> anyhow::Result<Self> {
        // Before loading: a stale "accelerator failed" memo from the old
        // provider would otherwise pin `auto` to CPU.
        super::reset_accel_failed();
        let mut detail = None;
        let selfie = reload(
            wanted.selfie,
            SelfieSeg::load_with_provider,
            provider,
            &mut detail,
        )?;
        let yunet = reload(
            wanted.yunet,
            YuNet::load_with_provider,
            provider,
            &mut detail,
        )?;
        let palm = reload(
            wanted.palm,
            PalmDetector::load_with_provider,
            provider,
            &mut detail,
        )?;
        let hand = reload(
            wanted.hand,
            HandLandmarker::load_with_provider,
            provider,
            &mut detail,
        )?;
        // Auto light only runs on the NPU, so a provider it can't use turns it
        // off (the lazy reload logs why) instead of failing the whole swap.
        let auto_light = reload(
            wanted.auto_light,
            AutoLight::load_with_provider,
            provider,
            &mut detail,
        )
        .unwrap_or_default();

        let mut eng = Self {
            generation: 0,
            provider: provider.to_string(),
            active: "cpu",
            detail,
            selfie: Slot::preloaded(selfie),
            yunet: Slot::preloaded(yunet),
            palm: Slot::preloaded(palm),
            hand: Slot::preloaded(hand),
            auto_light: Slot::preloaded(auto_light),
        };
        eng.update_active_status();
        Ok(eng)
    }

    pub fn loaded(&self) -> LoadedModels {
        LoadedModels {
            selfie: self.selfie.is_loaded(),
            yunet: self.yunet.is_loaded(),
            palm: self.palm.is_loaded(),
            hand: self.hand.is_loaded(),
            auto_light: self.auto_light.is_loaded(),
        }
    }

    /// Swap in `next` as the next generation. A model the old engine loaded
    /// after `next` was built is dropped with it and reloads lazily.
    pub fn replace(&mut self, next: Engine) {
        let generation = self.generation + 1;
        *self = next;
        self.generation = generation;
        super::reset_accel_failed();
    }

    pub fn update_active_status(&mut self) {
        let eps: Vec<&'static str> = [
            self.selfie.active_ep,
            self.yunet.active_ep,
            self.palm.active_ep,
            self.hand.active_ep,
            self.auto_light.active_ep,
        ]
        .into_iter()
        .flatten()
        .collect();
        self.active = match eps.split_first() {
            None => {
                let (active, detail) = super::predicted_active(&self.provider);
                self.detail = detail;
                active
            }
            Some((first, rest)) if rest.iter().all(|ep| ep == first) => first,
            Some(_) => "mixed",
        };
    }

    pub fn selfie(&mut self) -> Option<&mut SelfieSeg> {
        if self.selfie.model.is_none() && !self.selfie.failed {
            let mut fb = None;
            self.selfie.ensure_loaded(
                &self.provider,
                SelfieSeg::load_with_provider,
                "selfie segmentation unavailable; portrait blur / bg replace disabled",
                &mut fb,
            );
            if let Some(f) = fb {
                self.detail = Some(f);
            }
            self.update_active_status();
        }
        self.selfie.as_mut()
    }

    pub fn yunet(&mut self) -> Option<&mut YuNet> {
        if self.yunet.model.is_none() && !self.yunet.failed {
            let mut fb = None;
            self.yunet.ensure_loaded(
                &self.provider,
                YuNet::load_with_provider,
                "yunet unavailable; center stage cannot track a subject",
                &mut fb,
            );
            if let Some(f) = fb {
                self.detail = Some(f);
            }
            self.update_active_status();
        }
        self.yunet.as_mut()
    }

    pub fn palm(&mut self) -> Option<&mut PalmDetector> {
        if self.palm.model.is_none() && !self.palm.failed {
            let mut fb = None;
            self.palm.ensure_loaded(
                &self.provider,
                PalmDetector::load_with_provider,
                "palm detection unavailable; gesture reactions disabled",
                &mut fb,
            );
            if let Some(f) = fb {
                self.detail = Some(f);
            }
            self.update_active_status();
        }
        self.palm.as_mut()
    }

    pub fn hand(&mut self) -> Option<&mut HandLandmarker> {
        if self.hand.model.is_none() && !self.hand.failed {
            let mut fb = None;
            self.hand.ensure_loaded(
                &self.provider,
                HandLandmarker::load_with_provider,
                "hand landmark model unavailable; gesture reactions disabled",
                &mut fb,
            );
            if let Some(f) = fb {
                self.detail = Some(f);
            }
            self.update_active_status();
        }
        self.hand.as_mut()
    }

    pub fn auto_light(&mut self) -> Option<&mut AutoLight> {
        if self.auto_light.model.is_none() && !self.auto_light.failed {
            let mut fb = None;
            self.auto_light.ensure_loaded(
                &self.provider,
                AutoLight::load_with_provider,
                "auto light unavailable (needs an NPU and the iat_lowlight model)",
                &mut fb,
            );
            if let Some(f) = fb {
                self.detail = Some(f);
            }
            self.update_active_status();
        }
        self.auto_light.as_mut()
    }

    /// Drop every model unused for `max_idle`; true if any was freed.
    pub fn unload_idle(&mut self, max_idle: Duration) -> bool {
        let selfie = self.selfie.unload_idle(max_idle);
        let yunet = self.yunet.unload_idle(max_idle);
        let palm = self.palm.unload_idle(max_idle);
        let hand = self.hand.unload_idle(max_idle);
        let auto = self.auto_light.unload_idle(max_idle);
        let freed = selfie || yunet || palm || hand || auto;
        if freed {
            self.update_active_status();
        }
        freed
    }

    /// An engine whose every model has already failed, so nothing loads.
    #[cfg(test)]
    pub(crate) fn offline() -> Self {
        let mut eng = Self::default();
        eng.selfie.failed = true;
        eng.yunet.failed = true;
        eng.palm.failed = true;
        eng.hand.failed = true;
        eng.auto_light.failed = true;
        eng
    }
}

/// Load a model on `provider` if `wanted`, recording any CPU fallback reason.
fn reload<T>(
    wanted: bool,
    load: impl FnOnce(&str) -> anyhow::Result<(T, &'static str, Option<String>)>,
    provider: &str,
    detail: &mut Option<String>,
) -> anyhow::Result<Option<(T, &'static str)>> {
    if !wanted {
        return Ok(None);
    }
    let (model, ep, fallback) = load(provider)?;
    if fallback.is_some() {
        *detail = fallback;
    }
    Ok(Some((model, ep)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_slot_unloads_then_reloads_on_demand() {
        let mut slot: Slot<u32> = Slot::default();
        assert_eq!(slot.get_simple(|| Ok(7), "x"), Some(&mut 7));
        assert!(!slot.unload_idle(Duration::from_secs(60)));
        assert!(slot.unload_idle(Duration::ZERO));
        assert!(!slot.unload_idle(Duration::ZERO));
        assert_eq!(slot.get_simple(|| Ok(8), "x"), Some(&mut 8));
    }

    #[test]
    fn failed_slot_is_not_retried() {
        let mut slot: Slot<u32> = Slot::default();
        assert!(slot.get_simple(|| anyhow::bail!("missing"), "x").is_none());
        assert!(slot.get_simple(|| Ok(1), "x").is_none());
    }

    #[test]
    fn replace_bumps_generation_and_clears_failures() {
        let mut eng = Engine::offline();
        let fresh = Engine::load("cpu", eng.loaded()).expect("load on cpu");
        eng.replace(fresh);
        assert_eq!(eng.generation, 1);
        // The new engine retries models the old one had given up on.
        assert!(!eng.selfie.failed && !eng.auto_light.failed);
        assert_eq!((eng.provider.as_str(), eng.active), ("cpu", "cpu"));
        eng.replace(Engine::load("cpu", eng.loaded()).unwrap());
        assert_eq!(eng.generation, 2);
    }
}
