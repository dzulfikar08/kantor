//! The threads behind a PDF pane.
//!
//! hayro's documents aren't `Send`, so each thread opens the file itself
//! from shared bytes: the *render* thread reads the file, reports what it
//! found ([`Msg::Opened`] or [`Msg::Failed`]), then rasterizes pages and
//! thumbnails; the *text* thread extracts page text and links (search runs
//! through every page, which mustn't hold up drawing).
//!
//! The UI replaces the render queue with what it needs now each frame
//! ([`Worker::want_pages`]), so pages scrolled past are never drawn.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use gpui_kit::RenderImage;
use pdf_engine::{Link, OpenError, OutlineItem, PageText, PdfDoc, Renderer, Turn};

/// Pages drawn with one hayro cache before it is replaced.
const RENEW_CACHE: u32 = 48;

/// A page raster to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageJob {
    pub page: usize,
    /// Device pixels per point.
    pub scale: f32,
    pub turn: Turn,
}

/// What the threads report.
pub(crate) enum Msg {
    Opened {
        sizes: Vec<(f32, f32)>,
        outline: Vec<OutlineItem>,
        title: Option<String>,
    },
    Failed(OpenError),
    Page {
        job: PageJob,
        thumb: bool,
        image: Arc<RenderImage>,
        bytes: usize,
    },
    Text {
        page: usize,
        text: Arc<PageText>,
        links: Vec<Link>,
    },
}

#[derive(Default)]
struct Queues {
    pages: VecDeque<PageJob>,
    thumbs: VecDeque<PageJob>,
    texts: VecDeque<usize>,
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
            .spawn(move || render_thread(path, password, s, tx));
        if let Err(e) = spawned {
            log::error!("starting the PDF thread: {e}");
        }
        Self { shared }
    }

    /// The pages to draw now, most wanted first (replaces the queue).
    pub fn want_pages(&self, jobs: Vec<PageJob>) {
        self.replace(jobs, |q| &mut q.pages);
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
    pub fn want_text(&self, pages: impl IntoIterator<Item = usize>) {
        let Ok(mut q) = self.shared.q.lock() else {
            return;
        };
        q.texts.extend(pages);
        drop(q);
        self.shared.text.notify_all();
    }

    /// Move `page` to the front of the text queue (it's on screen).
    pub fn text_first(&self, page: usize) {
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

fn render_thread(
    path: PathBuf,
    password: String,
    shared: Arc<Shared>,
    tx: async_channel::Sender<Msg>,
) {
    let data = match std::fs::read(&path) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            log::warn!("reading {}: {e}", path.display());
            let _ = tx.send_blocking(Msg::Failed(OpenError::Damaged));
            return;
        }
    };
    let started = std::time::Instant::now();
    let doc = match PdfDoc::open(data.clone(), &password) {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send_blocking(Msg::Failed(e));
            return;
        }
    };
    let opened = Msg::Opened {
        sizes: doc.page_sizes(),
        outline: doc.outline(),
        title: doc.title(),
    };
    log::info!(
        "opened {} ({} pages) in {:.0} ms",
        path.display(),
        doc.page_count(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    if tx.send_blocking(opened).is_err() {
        return;
    }
    {
        let (shared, tx) = (shared.clone(), tx.clone());
        let spawned = std::thread::Builder::new()
            .name("pdf-text".into())
            .spawn(move || text_thread(data, password, shared, tx));
        if let Err(e) = spawned {
            log::error!("starting the PDF text thread: {e}");
        }
    }
    // hayro's cache keeps fonts and decoded images for as long as it
    // lives: a fresh one now and then bounds it in long files.
    let mut renderer = Renderer::new(&doc);
    let mut rendered = 0_u32;
    loop {
        let (job, thumb) = {
            let Ok(mut q) = shared.q.lock() else {
                return;
            };
            loop {
                if q.stop {
                    return;
                }
                if let Some(j) = q.pages.pop_front() {
                    break (j, false);
                }
                if let Some(j) = q.thumbs.pop_front() {
                    break (j, true);
                }
                q = match shared.render.wait(q) {
                    Ok(q) => q,
                    Err(_) => return,
                };
            }
        };
        rendered += 1;
        if rendered.is_multiple_of(RENEW_CACHE) {
            renderer = Renderer::new(&doc);
        }
        let Some(raster) = renderer.render(job.page, job.scale, job.turn) else {
            continue;
        };
        let bytes = raster.bytes();
        let Some(image) = to_image(raster) else {
            continue;
        };
        if tx
            .send_blocking(Msg::Page {
                job,
                thumb,
                image,
                bytes,
            })
            .is_err()
        {
            return;
        }
    }
}

fn text_thread(
    data: Arc<Vec<u8>>,
    password: String,
    shared: Arc<Shared>,
    tx: async_channel::Sender<Msg>,
) {
    let Ok(doc) = PdfDoc::open(data, &password) else {
        return;
    };
    let mut done = vec![false; doc.page_count()];
    loop {
        let page = {
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
        if done.get(page).is_none_or(|d| *d) {
            continue;
        }
        done[page] = true;
        let msg = Msg::Text {
            page,
            text: Arc::new(doc.text(page)),
            links: doc.links(page),
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
