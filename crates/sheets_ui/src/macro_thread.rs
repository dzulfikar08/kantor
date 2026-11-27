//! A workbook's macro thread: one per pane while content is enabled.
//!
//! The VBA project is built on `Rc` (not `Send`) and must stay alive between
//! runs, so module-level variables keep their values from one macro or event
//! handler to the next (`sheets_script::MacroEngine`). It lives on a thread
//! of its own, made at Enable Content and ended when the pane closes (its
//! job channel drops): each run or event is a [`Work`] item that carries the
//! workbook there and a channel back (`macro_run::ToUi`), which hands the
//! workbook back when the run ends. The UI thread never touches the
//! project; it keeps the project's [`EventIndex`] (plain data) to tell,
//! without asking the thread, whether an event has a handler at all.

use std::sync::mpsc;

use sheets_engine::Workbook;
use sheets_script::{Event, EventIndex, MacroEngine, MacroProject, RunOptions, View};

use crate::macro_run::{ChannelUi, ToUi};

/// What the thread runs.
pub(crate) enum MacroTask {
    /// A macro by name.
    Run(String),
    /// An event's handlers; `fold_from` joins them to the user's edit in
    /// one undo step.
    Fire {
        event: Event,
        fold_from: Option<usize>,
    },
    /// The first `Application.OnTime` schedule due at `now` (local OLE
    /// date), if any.
    Due { now: f64 },
}

pub(crate) struct Work {
    pub(crate) book: Workbook,
    pub(crate) task: MacroTask,
    pub(crate) view: View,
    pub(crate) options: RunOptions,
    pub(crate) tx: mpsc::Sender<ToUi>,
}

/// The UI's handle on a workbook's macro thread.
pub(crate) struct MacroThread {
    jobs: mpsc::Sender<Work>,
    pub(crate) index: EventIndex,
}

impl MacroThread {
    pub(crate) fn start(macros: MacroProject) -> Option<MacroThread> {
        let index = macros.event_index();
        let (jobs, rx) = mpsc::channel::<Work>();
        std::thread::Builder::new()
            .name("kantor-macros".into())
            // The interpreter recurses on the native stack.
            .stack_size(32 << 20)
            .spawn(move || thread_main(macros, rx))
            .map_err(|e| log::error!("starting the macro thread: {e}"))
            .ok()?;
        Some(MacroThread { jobs, index })
    }

    /// Hands `work` to the thread; gives it back if the thread is gone.
    pub(crate) fn send(&self, work: Work) -> Result<(), Work> {
        self.jobs.send(work).map_err(|mpsc::SendError(w)| w)
    }
}

fn thread_main(macros: MacroProject, rx: mpsc::Receiver<Work>) {
    let mut engine = MacroEngine::new(macros);
    // On the desktop, the workbook's own folder needs no asking.
    engine.set_paths(Vec::new(), true);
    for work in rx {
        let Work {
            book,
            task,
            view,
            options,
            tx,
        } = work;
        let ui = Box::new(ChannelUi { tx: tx.clone() });
        let (book, outcome, ran) = match task {
            MacroTask::Run(name) => {
                let (b, o) = engine.run(book, &name, view, ui, &options);
                (b, o, Some(name))
            }
            MacroTask::Fire { event, fold_from } => {
                let (b, o) = engine.fire(book, &event, view, ui, &options, fold_from);
                (b, o, None)
            }
            MacroTask::Due { now } => match engine.due(now) {
                Some(name) => {
                    let (b, o) = engine.run(book, &name, view, ui, &options);
                    (b, o, Some(name))
                }
                None => (book, sheets_script::RunOutcome::nothing(view), None),
            },
        };
        let next_timer = engine.timers().first().map(|t| t.at);
        let _ = tx.send(ToUi::Done {
            book,
            outcome,
            next_timer,
            ran,
        });
    }
}
