//! T039: Property-based invariants for the tab lifecycle state machine and
//! session snapshot round-trips (Constitution: crash-resistance).

use proptest::prelude::*;

use blaze_core::session::SessionSnapshot;
use blaze_core::tabs::{Rect, TabId, TabManager, TabState, WindowId};

#[derive(Debug, Clone)]
enum Op {
    CreateWindow,
    CreateTab(usize),
    CloseTab(usize),
    CloseWindow(usize),
    ActivateTab(usize),
    ReorderTab(usize, u32),
    MoveTab(usize, usize, u32),
    Pin(usize, bool),
    Reopen(usize),
    Transition(usize, TabState),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::CreateWindow),
        (0usize..8).prop_map(Op::CreateTab),
        (0usize..16).prop_map(Op::CloseTab),
        (0usize..4).prop_map(Op::CloseWindow),
        (0usize..16).prop_map(Op::ActivateTab),
        (0usize..16, 0u32..10).prop_map(|(t, p)| Op::ReorderTab(t, p)),
        (0usize..16, 0usize..4, 0u32..10).prop_map(|(t, w, p)| Op::MoveTab(t, w, p)),
        (0usize..16, any::<bool>()).prop_map(|(t, p)| Op::Pin(t, p)),
        (0usize..4).prop_map(Op::Reopen),
        (
            0usize..16,
            prop_oneof![
                Just(TabState::Active),
                Just(TabState::Loading),
                Just(TabState::Suspended),
                Just(TabState::Crashed),
            ]
        )
            .prop_map(|(t, s)| Op::Transition(t, s)),
    ]
}

fn nth_tab(mgr: &TabManager, n: usize) -> Option<TabId> {
    let mut ids: Vec<TabId> = mgr
        .windows()
        .flat_map(|w| w.tab_ids.iter().cloned())
        .collect();
    ids.sort();
    if ids.is_empty() {
        None
    } else {
        Some(ids[n % ids.len()].clone())
    }
}

fn nth_window(mgr: &TabManager, n: usize) -> Option<WindowId> {
    let mut ids: Vec<WindowId> = mgr.windows().map(|w| w.id.clone()).collect();
    ids.sort();
    if ids.is_empty() {
        None
    } else {
        Some(ids[n % ids.len()].clone())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// No sequence of tab/window operations may ever break structural
    /// invariants (valid focus, correct backlinks, no empty windows).
    #[test]
    fn tab_lifecycle_never_breaks_invariants(ops in prop::collection::vec(op_strategy(), 1..80)) {
        let mut mgr = TabManager::default();
        for op in ops {
            match op {
                Op::CreateWindow => {
                    mgr.create_window(Rect::default());
                }
                Op::CreateTab(w) => {
                    if let Some(w) = nth_window(&mgr, w) {
                        let _ = mgr.create_tab(&w, None);
                    }
                }
                Op::CloseTab(t) => {
                    if let Some(t) = nth_tab(&mgr, t) {
                        let _ = mgr.close_tab(&t);
                    }
                }
                Op::CloseWindow(w) => {
                    if let Some(w) = nth_window(&mgr, w) {
                        let _ = mgr.close_window(&w);
                    }
                }
                Op::ActivateTab(t) => {
                    if let Some(t) = nth_tab(&mgr, t) {
                        let _ = mgr.activate_tab(&t);
                    }
                }
                Op::ReorderTab(t, p) => {
                    if let Some(t) = nth_tab(&mgr, t) {
                        let _ = mgr.reorder_tab(&t, p);
                    }
                }
                Op::MoveTab(t, w, p) => {
                    if let (Some(t), Some(w)) = (nth_tab(&mgr, t), nth_window(&mgr, w)) {
                        let _ = mgr.move_tab(&t, &w, p);
                    }
                }
                Op::Pin(t, pinned) => {
                    if let Some(t) = nth_tab(&mgr, t) {
                        let _ = mgr.set_pinned(&t, pinned);
                    }
                }
                Op::Reopen(w) => {
                    if let Some(w) = nth_window(&mgr, w)
                        && let Some(closed) = mgr.reopen_closed_tab(&w) {
                            let _ = mgr.create_tab(&w, Some(closed.url));
                        }
                }
                Op::Transition(t, s) => {
                    if let Some(t) = nth_tab(&mgr, t) {
                        let _ = mgr.transition(&t, s); // illegal moves must be rejected, not corrupt
                    }
                }
            }
            mgr.check_invariants().map_err(TestCaseError::fail)?;
        }
    }

    /// Session snapshots must round-trip losslessly through JSON.
    #[test]
    fn snapshot_json_round_trips(
        urls in prop::collection::vec("[a-z]{1,12}", 1..10),
        pinned in prop::collection::vec(any::<bool>(), 10),
        active in 0usize..10,
    ) {
        let mut mgr = TabManager::default();
        let tabs: Vec<_> = urls
            .iter()
            .zip(&pinned)
            .map(|(u, p)| {
                (format!("https://{u}.example/"), u.to_uppercase(), *p, Default::default())
            })
            .collect();
        let count = tabs.len();
        mgr.restore_window(Rect::default(), tabs, active % count).unwrap();

        let snapshot = snapshot_of(&mgr);
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: SessionSnapshot = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        prop_assert_eq!(parsed.windows.len(), 1);
        prop_assert_eq!(parsed.windows[0].tabs.len(), count);
        prop_assert_eq!(parsed.windows[0].active_index, active % count);
    }
}

fn snapshot_of(mgr: &TabManager) -> SessionSnapshot {
    use blaze_core::session::{TabSnapshot, WindowSnapshot};
    SessionSnapshot {
        windows: mgr
            .windows()
            .map(|w| WindowSnapshot {
                frame: w.frame,
                active_index: w
                    .tab_ids
                    .iter()
                    .position(|t| *t == w.active_tab_id)
                    .unwrap_or(0),
                tabs: w
                    .tab_ids
                    .iter()
                    .filter_map(|id| mgr.tab(id).ok())
                    .map(|t| TabSnapshot {
                        url: t.url.clone(),
                        title: t.title.clone(),
                        pinned: t.pinned,
                        history: t.history.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

// ---- T052: download state machine properties ----

mod downloads {
    use super::*;
    use blaze_core::downloads::DownloadState;

    const ALL: [DownloadState; 5] = [
        DownloadState::Active,
        DownloadState::Paused,
        DownloadState::Completed,
        DownloadState::Interrupted,
        DownloadState::Cancelled,
    ];

    fn state_strategy() -> impl Strategy<Value = DownloadState> {
        prop::sample::select(ALL.as_slice())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// Applying arbitrary transition attempts never violates the
        /// data-model rules: terminal states are absorbing, self-loops are
        /// rejected, and every accepted edge is in the documented matrix.
        #[test]
        fn download_transitions_respect_matrix(
            targets in prop::collection::vec(state_strategy(), 1..64)
        ) {
            let mut state = DownloadState::Active;
            for to in targets {
                let allowed = state.can_transition(to);

                prop_assert!(!(allowed && state.is_terminal()),
                    "terminal {state:?} accepted -> {to:?}");
                prop_assert!(!(allowed && state == to),
                    "self-loop accepted at {state:?}");
                if allowed {
                    // Every accepted edge is one of the documented ones.
                    use DownloadState::*;
                    prop_assert!(matches!(
                        (state, to),
                        (Active, Paused) | (Active, Completed)
                            | (Active, Interrupted) | (Active, Cancelled)
                            | (Paused, Active) | (Paused, Cancelled)
                            | (Interrupted, Active) | (Interrupted, Cancelled)
                    ));
                    state = to;
                }
            }
            // Whatever the walk, we end in a parseable, round-trippable state.
            prop_assert_eq!(DownloadState::parse(state.as_str()), Some(state));
        }

        /// From any state, a resume (-> Active) is possible iff the state is
        /// Paused or Interrupted — the exact user-facing rule (FR-022/023).
        #[test]
        fn resume_only_from_paused_or_interrupted(from in state_strategy()) {
            let resumable = matches!(
                from,
                DownloadState::Paused | DownloadState::Interrupted
            );
            prop_assert_eq!(from.can_transition(DownloadState::Active), resumable);
        }
    }
}
