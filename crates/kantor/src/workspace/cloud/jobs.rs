//! Running blocking network work (WebDAV, the keychain) on worker threads.
//!
//! GPUI's background executor is a small shared pool; a save on a slow link
//! takes minutes, so each job gets its own thread and reports back over a
//! channel. Results and progress are applied on the UI thread.

use gpui_kit::*;

enum Msg<P, T> {
    Progress(P),
    Done(T),
}

/// Run `job` on a worker thread. It may report progress through the
/// function it's given; `on_progress` and `on_done` run on the UI thread
/// while the view is alive.
pub(crate) fn spawn<V: 'static, P: Send + 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    job: impl FnOnce(&dyn Fn(P)) -> T + Send + 'static,
    mut on_progress: impl FnMut(&mut V, P, &mut Context<V>) + 'static,
    on_done: impl FnOnce(&mut V, T, &mut Context<V>) + 'static,
) {
    let rx = start(job);
    let mut on_done = Some(on_done);
    cx.spawn(async move |this, cx| {
        while let Ok(msg) = rx.recv().await {
            let alive = this.update(cx, |view, cx| match msg {
                Msg::Progress(p) => on_progress(view, p, cx),
                Msg::Done(t) => {
                    if let Some(f) = on_done.take() {
                        f(view, t, cx);
                    }
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// [`spawn`] for callbacks that need the window (opening documents,
/// dialogs).
pub(crate) fn spawn_in<V: 'static, P: Send + 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    window: &mut Window,
    job: impl FnOnce(&dyn Fn(P)) -> T + Send + 'static,
    mut on_progress: impl FnMut(&mut V, P, &mut Window, &mut Context<V>) + 'static,
    on_done: impl FnOnce(&mut V, T, &mut Window, &mut Context<V>) + 'static,
) {
    let rx = start(job);
    let mut on_done = Some(on_done);
    cx.spawn_in(window, async move |this, cx| {
        while let Ok(msg) = rx.recv().await {
            let alive = this.update_in(cx, |view, window, cx| match msg {
                Msg::Progress(p) => on_progress(view, p, window, cx),
                Msg::Done(t) => {
                    if let Some(f) = on_done.take() {
                        f(view, t, window, cx);
                    }
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

fn start<P: Send + 'static, T: Send + 'static>(
    job: impl FnOnce(&dyn Fn(P)) -> T + Send + 'static,
) -> async_channel::Receiver<Msg<P, T>> {
    let (tx, rx) = async_channel::unbounded::<Msg<P, T>>();
    let spawned = std::thread::Builder::new()
        .name("kantor-cloud".into())
        .spawn(move || {
            let progress_tx = tx.clone();
            let result = job(&move |p| {
                let _ = progress_tx.try_send(Msg::Progress(p));
            });
            let _ = tx.send_blocking(Msg::Done(result));
        });
    if let Err(e) = spawned {
        log::error!("can't start a cloud worker: {e}");
    }
    rx
}
