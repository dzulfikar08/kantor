//! The threads behind a PDF pane.
//!
//! hayro's documents aren't `Send`, so each thread opens the files itself
//! from shared bytes: the *render* thread reads the file, reports what it
//! found ([`Msg::Opened`] or [`Msg::Failed`]), then rasterizes pages and
//! thumbnails; the *text* thread extracts page text and links (search runs
//! through every page, which mustn't hold up drawing).
//!
//! An edited document has pages from several sources ([`crate::model`]):
//! jobs name a page by source and index ([`PageKey`]), and each thread opens
//! a source the first time it needs it.
//!
//! The UI replaces the render queue with what it needs now each frame
//! ([`Worker::want_pages`]), so pages scrolled past are never drawn.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use gpui_kit::RenderImage;
use pdf_engine::{Link, OpenError, PageText, PdfDoc, Renderer, Turn};

use crate::model::{PageKey, Source, SrcId};

/// Pages drawn with one hayro cache before it is replaced.
const RENEW_CACHE: u32 = 48;

/// A page raster to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageJob {
    pub key: PageKey,
    /// Device pixels per point.
    pub scale: f32,
    /// The view's rotation plus the page's own extra rotation.
    pub turn: Turn,
}

/// What the threads report.
pub(crate) enum Msg {
    Opened(Arc<Source>),
    Failed(OpenError),
    Page {
        job: PageJob,
        thumb: bool,
        image: Arc<RenderImage>,
        bytes: usize,
    },
    Text {
        key: PageKey,
        text: Arc<PageText>,
        links: Vec<Link>,
    },
}

#[derive(Default)]
struct Queues {
    pages: VecDeque<PageJob>,
    /// The Organize Pages grid's thumbnails.
    grid: VecDeque<PageJob>,
    thumbs: VecDeque<PageJob>,
    texts: VecDeque<PageKey>,
    sources: HashMap<SrcId, Arc<Source>>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    q: Mutex<Queues>,
    /// Wakes the render thread.
    render: Condvar,
    /// Wakes the text thread.
    text: Condvar,
}

/// The threads of one open file; dropping it stops them.
pub(crate) struct Worker {
    shared: Arc<Shared>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut q) = self.shared.q.lock() {
            q.stop = true;
        }
        self.shared.render.notify_all();
        self.shared.text.notify_all();
    }
}

impl Worker {
    /// Open `path` with `password` (`""` for none) and start the threads.
    pub fn start(path: PathBuf, password: String, tx: async_channel::Sender<Msg>) -> Self {
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || render_thread(Load::Path(path), password, s, tx));
        if let Err(e) = spawned {
            log::error!("starting the PDF thread: {e}");
        }
        Self { shared }
    }

    /// Start the threads on `bytes` already in memory (a recovery copy).
    pub fn start_bytes(
        name: String,
        bytes: Arc<Vec<u8>>,
        password: String,
        tx: async_channel::Sender<Msg>,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || render_thread(Load::Bytes(name, bytes), password, s, tx));
        if let Err(e) = spawned {
            log::error!("starting the PDF thread: {e}");
        }
        Self { shared }
    }

    /// Make `source`'s pages available to draw.
    pub fn add_source(&self, source: Arc<Source>) {
        if let Ok(mut q) = self.shared.q.lock() {
            q.sources.insert(source.id, source);
        }
    }

    /// The pages to draw now, most wanted first (replaces the queue).
    pub fn want_pages(&self, jobs: Vec<PageJob>) {
        self.replace(jobs, |q| &mut q.pages);
    }

    /// The grid's thumbnails to draw now (replaces their queue).
    pub fn want_grid(&self, jobs: Vec<PageJob>) {
        self.replace(jobs, |q| &mut q.grid);
    }

    /// The thumbnails to draw now (replaces their queue).
    pub fn want_thumbs(&self, jobs: Vec<PageJob>) {
        self.replace(jobs, |q| &mut q.thumbs);
    }

    fn replace(&self, jobs: Vec<PageJob>, which: fn(&mut Queues) -> &mut VecDeque<PageJob>) {
        let Ok(mut q) = self.shared.q.lock() else {
            return;
        };
        let queue = which(&mut q);
        if queue.iter().eq(jobs.iter()) {
            return;
        }
        *queue = jobs.into();
        drop(q);
        self.shared.render.notify_all();
    }

    /// Extract the text of `pages` (in this order, after those asked before).
    pub fn want_text(&self, pages: impl IntoIterator<Item = PageKey>) {
        let Ok(mut q) = self.shared.q.lock() else {
            return;
        };
        q.texts.extend(pages);
        drop(q);
        self.shared.text.notify_all();
    }

    /// Move `page` to the front of the text queue (it's on screen).
    pub fn text_first(&self, page: PageKey) {
        let Ok(mut q) = self.shared.q.lock() else {
            return;
        };
        if let Some(i) = q.texts.iter().position(|&p| p == page) {
            q.texts.remove(i);
        }
        q.texts.push_front(page);
        drop(q);
        self.shared.text.notify_all();
    }
}

enum Load {
    Path(PathBuf),
    Bytes(String, Arc<Vec<u8>>),
}

fn render_thread(
    load: Load,
    password: String,
    shared: Arc<Shared>,
    tx: async_channel::Sender<Msg>,
) {
    let (name, data) = match load {
        Load::Path(path) => match std::fs::read(&path) {
            Ok(d) => (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                Arc::new(d),
            ),
            Err(e) => {
                log::warn!("reading {}: {e}", path.display());
                let _ = tx.send_blocking(Msg::Failed(OpenError::Damaged));
                return;
            }
        },
        Load::Bytes(name, bytes) => (name, bytes),
    };
    let started = std::time::Instant::now();
    let source = match Source::read(name, data, password) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            let _ = tx.send_blocking(Msg::Failed(e));
            return;
        }
    };
    log::info!(
        "opened {} ({} pages) in {:.0} ms",
        source.name,
        source.sizes.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    if let Ok(mut q) = shared.q.lock() {
        q.sources.insert(source.id, source.clone());
    }
    if tx.send_blocking(Msg::Opened(source)).is_err() {
        return;
    }
    {
        let (shared, tx) = (shared.clone(), tx.clone());
        let spawned = std::thread::Builder::new()
            .name("pdf-text".into())
            .spawn(move || text_thread(shared, tx));
        if let Err(e) = spawned {
            log::error!("starting the PDF text thread: {e}");
        }
    }
    let mut docs: HashMap<SrcId, PdfDoc> = HashMap::new();
    let mut next: Option<(PageJob, bool)> = None;
    loop {
        let Some((job, thumb)) = next.take().or_else(|| next_render(&shared)) else {
            return;
        };
        let Some(doc) = open_source(&shared, &mut docs, job.key.0) else {
            continue;
        };
        match render_source(doc, (job, thumb), &shared, &tx) {
            Some(other) => next = Some(other),
            None => {
                if stopped(&shared) {
                    return;
                }
            }
        }
    }
}

fn stopped(shared: &Shared) -> bool {
    shared.q.lock().map_or(true, |q| q.stop)
}

/// The next page to draw (pages before thumbnails); waits for one. `None`
/// when the pane is gone.
fn next_render(shared: &Shared) -> Option<(PageJob, bool)> {
    let mut q = shared.q.lock().ok()?;
    loop {
        if q.stop {
            return None;
        }
        if let Some(j) = q.pages.pop_front() {
            return Some((j, false));
        }
        if let Some(j) = q.grid.pop_front() {
            return Some((j, true));
        }
        if let Some(j) = q.thumbs.pop_front() {
            return Some((j, true));
        }
        q = shared.render.wait(q).ok()?;
    }
}

/// The open document of source `id`, opening it the first time.
fn open_source<'a>(
    shared: &Shared,
    docs: &'a mut HashMap<SrcId, PdfDoc>,
    id: SrcId,
) -> Option<&'a PdfDoc> {
    if let std::collections::hash_map::Entry::Vacant(slot) = docs.entry(id) {
        let source = shared.q.lock().ok()?.sources.get(&id).cloned()?;
        match PdfDoc::open(source.bytes.clone(), &source.password) {
            Ok(d) => {
                slot.insert(d);
            }
            Err(e) => {
                log::warn!("opening {} for drawing: {e}", source.name);
                return None;
            }
        }
    }
    docs.get(&id)
}

/// Draw `first` and the jobs after it while they are of the same source
/// (keeping hayro's cache); returns the first job of another source, or
/// `None` when the pane is gone or the channel closed.
fn render_source(
    doc: &PdfDoc,
    first: (PageJob, bool),
    shared: &Shared,
    tx: &async_channel::Sender<Msg>,
) -> Option<(PageJob, bool)> {
    // hayro's cache keeps fonts and decoded images for as long as it
    // lives: a fresh one now and then bounds it in long files.
    let mut renderer = Renderer::new(doc);
    let mut rendered = 0_u32;
    let src = first.0.key.0;
    let mut job = Some(first);
    loop {
        let (j, thumb) = match job.take() {
            Some(j) => j,
            None => next_render(shared)?,
        };
        if j.key.0 != src {
            return Some((j, thumb));
        }
        rendered += 1;
        if rendered.is_multiple_of(RENEW_CACHE) {
            renderer = Renderer::new(doc);
        }
        let Some(raster) = renderer.render(j.key.1, j.scale, j.turn) else {
            continue;
        };
        let bytes = raster.bytes();
        let Some(image) = to_image(raster) else {
            continue;
        };
        let msg = Msg::Page {
            job: j,
            thumb,
            image,
            bytes,
        };
        if tx.send_blocking(msg).is_err() {
            return None;
        }
    }
}

fn text_thread(shared: Arc<Shared>, tx: async_channel::Sender<Msg>) {
    let mut docs: HashMap<SrcId, PdfDoc> = HashMap::new();
    let mut done = std::collections::HashSet::new();
    loop {
        let key = {
            let Ok(mut q) = shared.q.lock() else {
                return;
            };
            loop {
                if q.stop {
                    return;
                }
                if let Some(p) = q.texts.pop_front() {
                    break p;
                }
                q = match shared.text.wait(q) {
                    Ok(q) => q,
                    Err(_) => return,
                };
            }
        };
        if !done.insert(key) {
            continue;
        }
        let Some(doc) = open_source(&shared, &mut docs, key.0) else {
            continue;
        };
        if key.1 >= doc.page_count() {
            continue;
        }
        let msg = Msg::Text {
            key,
            text: Arc::new(doc.text(key.1)),
            links: doc.links(key.1),
        };
        if tx.send_blocking(msg).is_err() {
            return;
        }
    }
}

/// A raster as an image GPUI can paint (BGRA), made off the UI thread.
fn to_image(r: pdf_engine::Raster) -> Option<Arc<RenderImage>> {
    let mut bytes = r.rgba;
    for c in bytes.as_chunks_mut::<4>().0 {
        c.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(r.width, r.height, bytes)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}
