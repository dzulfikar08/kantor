//! In-app updates (see `kantor_update`): a check at launch at most once a
//! day and from the palette, a background download, and a quiet status-bar
//! notice once the new build is ready. The swap happens on "Restart to
//! update" (after the unsaved-changes check) or when Kantor quits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_ui::i18n::{t, tf};
use kantor_update::http::Http;
use kantor_update::verify::SystemVerifier;
use kantor_update::{Channel, Config, Error, Staged, State};

use super::Workspace;
use super::closing::Closing;

const UPDATE_READY: &str = kantor_ui::i18n::tk("Kantor {} is ready — Restart to update");
const UPDATE_READY_CHOOSE: &str =
    kantor_ui::i18n::tk("Kantor {} is ready: choose Restart to Update");

#[derive(Default)]
pub(super) enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
    },
    /// Downloaded and verified; swapped in on restart or quit.
    Ready(Staged),
    /// Newer build Kantor can't install itself (package install, read-only
    /// location, failed download): offer the release page.
    Available {
        version: String,
        url: String,
    },
}

#[derive(Default)]
pub(super) struct Updates {
    pub(super) status: UpdateStatus,
    /// Bytes downloaded and expected, written by the download thread.
    progress: Arc<(AtomicU64, AtomicU64)>,
    /// The running check was asked for from the palette: report every
    /// outcome, errors included.
    manual: bool,
    /// `stable channel` or `updates off`, from settings.toml.
    setting: String,
}

fn staged_display(s: &Staged) -> String {
    kantor_update::BuildId::new(&s.version, s.build)
        .map(|b| b.display())
        .unwrap_or_else(|_| s.version.clone())
}

fn channel(settings: &kantor_core::settings::UpdateSettings) -> Channel {
    Channel::from_setting(settings.channel.as_deref())
}

impl Updates {
    /// The clickable status-bar text, if there is something to say.
    pub(super) fn status_text(&self) -> Option<String> {
        match &self.status {
            UpdateStatus::Ready(s) => Some(kantor_ui::i18n::fill(
                kantor_ui::i18n::t(UPDATE_READY),
                &[&staged_display(s)],
            )),
            UpdateStatus::Available { version, .. } => {
                Some(tf("Kantor {} is available — Download", &[version]))
            }
            UpdateStatus::Downloading { version } if self.manual => {
                let (done, total) = (
                    self.progress.0.load(Ordering::Relaxed),
                    self.progress.1.load(Ordering::Relaxed),
                );
                let pct = (done * 100).checked_div(total).unwrap_or(0);
                Some(tf("Downloading Kantor {}… {}%", &[version, &pct]))
            }
            UpdateStatus::Checking if self.manual => Some(t("Checking for updates…").into()),
            _ => None,
        }
    }

    /// The palette's footer line: this build and what the updater knows.
    pub(super) fn about_line(&self) -> String {
        let me = kantor_update::current_build().display();
        let state = match &self.status {
            UpdateStatus::Ready(s) => tf("{} ready, restart to update", &[&staged_display(s)]),
            UpdateStatus::Available { version, .. } => tf("{} available", &[version]),
            UpdateStatus::Downloading { version } => tf("downloading {}", &[version]),
            UpdateStatus::Checking => t("checking for updates").into(),
            UpdateStatus::Idle => self.setting.clone(),
        };
        format!("Kantor {me} · {state}")
    }

    fn remember(&mut self, settings: &kantor_core::settings::UpdateSettings) {
        self.setting = if settings.enabled {
            tf("{} channel", &[&channel(settings).as_str()])
        } else {
            t("updates off").into()
        };
    }
}

impl Workspace {
    /// At launch: pick up an update staged by an earlier run, then check if
    /// a day has passed since the last check.
    pub(super) fn init_updates(&mut self, cx: &mut Context<Self>) {
        let settings = Settings::load().update;
        self.updates.remember(&settings);
        let mut state = State::load();
        kantor_update::clean_leftovers(
            &kantor_update::InstallKind::detect(),
            state.staged.as_ref(),
        );
        if let Some(staged) = state.staged.take() {
            if settings.enabled
                && kantor_update::staged_is_current(&staged, &kantor_update::running_build())
            {
                self.updates.status = UpdateStatus::Ready(staged);
                return;
            }
            // Applied by hand, older than this build, or switched off.
            kantor_update::discard(&staged);
            state.save();
        }
        if !settings.enabled || std::env::var_os("KANTOR_BENCH_PALETTE").is_some() {
            return;
        }
        if kantor_update::state::due(state.last_check, kantor_update::state::now()) {
            self.check_for_updates(false, cx);
        }
    }

    /// Check now (palette: `manual`); download in the background when there
    /// is something newer.
    pub(super) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        let settings = Settings::load().update;
        self.updates.remember(&settings);
        if !settings.enabled {
            if manual {
                self.notice(
                    t("Updates are off ([update] enabled = false in settings.toml)").into(),
                    cx,
                );
            }
            return;
        }
        match &self.updates.status {
            UpdateStatus::Checking | UpdateStatus::Downloading { .. } => {
                if manual {
                    self.updates.manual = true;
                    cx.notify();
                }
                return;
            }
            UpdateStatus::Ready(s) if manual => {
                let text = kantor_ui::i18n::fill(
                    kantor_ui::i18n::t(UPDATE_READY_CHOOSE),
                    &[&staged_display(s)],
                );
                self.notice(text, cx);
                return;
            }
            UpdateStatus::Ready(_) => return,
            _ => {}
        }
        let mut state = State::load();
        state.last_check = Some(kantor_update::state::now());
        state.save();

        let cfg = Config::for_this_build(channel(&settings));
        log::info!(
            "update check: {} channel, {} install, {}",
            cfg.channel.as_str(),
            cfg.kind.describe(),
            cfg.manifest_url
        );
        self.updates.manual = manual;
        self.updates.status = UpdateStatus::Checking;
        self.updates.progress.0.store(0, Ordering::Relaxed);
        self.updates.progress.1.store(0, Ordering::Relaxed);
        cx.notify();
        let progress = self.updates.progress.clone();

        cx.spawn(async move |this, cx| {
            let check_cfg = cfg.clone();
            let checked = cx
                .background_executor()
                .spawn(async move { kantor_update::check(&check_cfg, &Http::new()) })
                .await;
            let available = match checked {
                Ok(Some(a)) => a,
                Ok(None) => {
                    let _ = this.update(cx, |ws, cx| {
                        ws.updates.status = UpdateStatus::Idle;
                        if ws.updates.manual {
                            let me = kantor_update::current_build().display();
                            ws.notice(tf("Kantor {} is up to date", &[&me]), cx);
                        }
                        cx.notify();
                    });
                    return;
                }
                Err(e) => {
                    log::warn!("update check failed: {e}");
                    let _ = this.update(cx, |ws, cx| {
                        ws.updates.status = UpdateStatus::Idle;
                        if ws.updates.manual {
                            ws.notice(tf("Update check failed: {}", &[&e]), cx);
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let version = available.display_version();
            let url = available.manifest.notes_url.clone();
            if available.asset.is_none() {
                log::info!(
                    "update: {version} is available ({} install: notice only)",
                    cfg.kind.describe()
                );
                let _ = this.update(cx, |ws, cx| {
                    ws.updates.status = UpdateStatus::Available { version, url };
                    cx.notify();
                });
                return;
            }
            let _ = this.update(cx, |ws, cx| {
                ws.updates.status = UpdateStatus::Downloading {
                    version: version.clone(),
                };
                cx.notify();
            });
            // Redraw the percentage while a manual download runs.
            let ticker = this.clone();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let going = ticker.update(cx, |ws, cx| {
                        let going = matches!(ws.updates.status, UpdateStatus::Downloading { .. });
                        if going && ws.updates.manual {
                            cx.notify();
                        }
                        going
                    });
                    if !matches!(going, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();

            let staged = cx
                .background_executor()
                .spawn(async move {
                    kantor_update::stage(
                        &cfg,
                        &available,
                        &Http::new(),
                        &SystemVerifier,
                        &mut |done, total| {
                            progress.0.store(done, Ordering::Relaxed);
                            progress.1.store(total, Ordering::Relaxed);
                        },
                    )
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                match staged {
                    Ok(staged) => {
                        log::info!("update: {version} downloaded and verified");
                        let mut state = State::load();
                        state.staged = Some(staged.clone());
                        state.save();
                        ws.updates.status = UpdateStatus::Ready(staged);
                    }
                    Err(e) => {
                        log::warn!("update: {version} not installed: {e}");
                        if ws.updates.manual || matches!(e, Error::NotWritable(_)) {
                            ws.notice(tf("Couldn't install Kantor {}: {}", &[&version, &e]), cx);
                        }
                        ws.updates.status = UpdateStatus::Available { version, url };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The status-bar notice was clicked.
    pub(super) fn update_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.updates.status {
            UpdateStatus::Ready(_) => self.restart_to_update(window, cx),
            UpdateStatus::Available { url, .. } => cx.open_url(url),
            _ => {}
        }
    }

    /// Restart into the downloaded build, once nothing unsaved would be lost.
    pub(super) fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.updates.status, UpdateStatus::Ready(_)) {
            self.request_close(Closing::Restart, window, cx);
        } else {
            self.notice(
                t("No update is ready. Use Check for Updates… first.").into(),
                cx,
            );
        }
    }

    /// Swap the staged build in and start it. `false` (and a notice) when
    /// the swap failed: the old build is intact and keeps running.
    pub(super) fn apply_update_and_relaunch(&mut self, cx: &mut Context<Self>) -> bool {
        let UpdateStatus::Ready(staged) = std::mem::take(&mut self.updates.status) else {
            return false;
        };
        match apply_staged(&staged, true) {
            Ok(()) => true,
            Err(e) => {
                self.notice(tf("Update failed, nothing changed: {}", &[&e]), cx);
                self.updates.status = UpdateStatus::Available {
                    version: staged_display(&staged),
                    url: staged.notes_url.clone(),
                };
                cx.notify();
                false
            }
        }
    }

    /// Kantor is quitting: put a downloaded update in place for next time.
    pub fn apply_update_on_quit(&mut self) {
        if let UpdateStatus::Ready(staged) = std::mem::take(&mut self.updates.status)
            && let Err(e) = apply_staged(&staged, false)
        {
            log::error!("update on quit failed (old build kept): {e}");
        }
    }
}

/// Swap in (and with `relaunch` start) and forget the staged update; a
/// failed swap is rolled back and its files dropped, so the next check starts
/// clean. Windows installer installs start the installer here instead.
fn apply_staged(staged: &Staged, relaunch: bool) -> Result<(), Error> {
    let result = if relaunch {
        kantor_update::apply_and_relaunch(staged)
    } else {
        kantor_update::apply(staged)
    };
    match &result {
        Ok(()) => log::info!("update: installed Kantor {}", staged.version),
        Err(e) => {
            log::error!("update: {e}");
            kantor_update::discard(staged);
        }
    }
    let mut state = State::load();
    state.staged = None;
    state.save();
    result
}
