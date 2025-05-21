use nomt::hasher::BinaryHasher;
use nomt::{Nomt, Options, SessionParams, WitnessMode};
use sov_rollup_interface::reexports::digest;

/// Contains all the most recent rollup data.
pub struct StateDb<H> {
    user: Nomt<BinaryHasher<H>>,
    kernel: Nomt<BinaryHasher<H>>,
    // TODO: Add a rocksdb for historical data. Will be done in follow up PR.
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

    pub(crate) fn commit(&self, overlays: StateOverlay) -> anyhow::Result<()> {
        let StateOverlay { user, kernel } = overlays;
        // TODO: Fail of kernel commit will leave user data inconsistent.
        //   to be addressed in follow up: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2634
        user.commit(&self.user)?;
        kernel.commit(&self.kernel)?;
        Ok(())
    }

    pub(crate) fn begin_session(
        &self,
        overlays: Vec<&StateOverlay>,
    ) -> anyhow::Result<StateSession<H>> {
        let user_overlays = overlays.iter().map(|o| &o.user);
        let user_session = Self::begin_single_session(&self.user, user_overlays)?;

        let kernel_overlays = overlays.iter().map(|o| &o.kernel);
        let kernel_session = Self::begin_single_session(&self.kernel, kernel_overlays)?;

        Ok(StateSession {
            user_session,
            kernel_session,
        })
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
    pub user_session: nomt::Session<BinaryHasher<H>>,
    #[allow(missing_docs)]
    pub kernel_session: nomt::Session<BinaryHasher<H>>,
}

pub(crate) struct StateOverlay {
    pub(crate) user: nomt::Overlay,
    pub(crate) kernel: nomt::Overlay,
}

// impl From<NomtStateChangeSet> for StateOverlay {
// //     fn from(value: NomtStateChangeSet) -> Self {
// //         let NomtStateChangeSet { user, kernel } = value;
// //         Self {
// //             user: user.into_overlay(),
// //             kernel: kernel.into_overlay(),
// //         }
// //     }
// // }

/// All non-path-related options, tuned for optimal performance in sov-rollup
pub(crate) fn sov_nomt_default_options() -> Options {
    let mut opts = Options::new();
    // Draft values, needs to be benchmarked on the target system type.
    opts.commit_concurrency(2);
    opts.prepopulate_page_cache(true);
    opts
}
