use nomt::hasher::BinaryHasher;
use nomt::{Nomt, Options, SessionParams, WitnessMode};
use sov_rollup_interface::reexports::digest;

/// Contains all the most recent rollup data.
pub struct StateDb<H> {
    user: Nomt<BinaryHasher<H>>,
    kernel: Nomt<BinaryHasher<H>>,
}

impl<H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync> StateDb<H> {
    /// Initialize a new [` StateDb `] in the given path.
    pub fn new(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let user = {
            let mut opts = sov_nomt_default_options();
            opts.path(path.as_ref().join("user_nomt_db"));
            Nomt::<BinaryHasher<H>>::open(opts)?
        };
        let kernel = {
            let mut opts = sov_nomt_default_options();
            opts.path(path.as_ref().join("kernel_nomt_db"));
            Nomt::<BinaryHasher<H>>::open(opts)?
        };

        Ok(Self { user, kernel })
    }

    /// Commit [`StateOverlay`] to disk.
    pub(crate) fn commit(&self, overlay: StateOverlay) -> anyhow::Result<()> {
        let StateOverlay { user, kernel } = overlay;
        // TODO: Fail of kernel commit will leave user data inconsistent.
        //   to be addressed in follow up: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2634
        user.commit(&self.user)?;
        kernel.commit(&self.kernel)?;
        Ok(())
    }

    /// Commit [`crate::storage_manager::StateFinishedSession`] to disk.
    #[cfg(feature = "test-utils")]
    pub fn commit_change_set(
        &self,
        session: crate::storage_manager::StateFinishedSession,
    ) -> anyhow::Result<()> {
        let overlay = session.into_state_overlay();
        self.commit(overlay)
    }

    /// Begin a new user and kernel session with a given list of overlays.
    pub(crate) fn begin_session(
        &self,
        overlays: Vec<&StateOverlay>,
    ) -> anyhow::Result<StateSession<H>> {
        let user_overlays = overlays.iter().map(|o| &o.user);
        let user_session = Self::begin_single_session(&self.user, user_overlays)?;

        let kernel_overlays = overlays.iter().map(|o| &o.kernel);
        let kernel_session = Self::begin_single_session(&self.kernel, kernel_overlays)?;

        Ok(StateSession {
            user: user_session,
            kernel: kernel_session,
        })
    }

    /// Begin a new user and kernel session with only data that has been written to disk
    #[cfg(feature = "test-utils")]
    pub fn begin_session_from_committed(&self) -> anyhow::Result<StateSession<H>> {
        self.begin_session(Vec::new())
    }

    fn begin_single_session<'a>(
        nomt: &Nomt<BinaryHasher<H>>,
        overlays: impl IntoIterator<Item = &'a nomt::Overlay>,
    ) -> anyhow::Result<nomt::Session<BinaryHasher<H>>> {
        let params = SessionParams::default()
            .overlay(overlays)
            .map_err(|e| anyhow::anyhow!("{:?}", e))?
            .witness_mode(WitnessMode::read_write());
        Ok(nomt.begin_session(params))
    }
}

/// This is a session used by actual storages. It can rely on several overlays plus actual NOMT.
pub struct StateSession<H> {
    #[allow(missing_docs)]
    pub user: nomt::Session<BinaryHasher<H>>,
    #[allow(missing_docs)]
    pub kernel: nomt::Session<BinaryHasher<H>>,
}

/// Combination of [`nomt::Overlay`] for user and kernel namespaces.
pub(crate) struct StateOverlay {
    pub(crate) user: nomt::Overlay,
    pub(crate) kernel: nomt::Overlay,
}

/// All non-path-related options, tuned for optimal performance in sov-rollup
pub(crate) fn sov_nomt_default_options() -> Options {
    let mut opts = Options::new();
    // Draft values, needs to be benchmarked on the target system type.
    opts.commit_concurrency(2);
    opts.prepopulate_page_cache(true);
    opts
}
