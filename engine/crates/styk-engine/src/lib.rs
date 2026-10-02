//! The engine facade.
//!
//! One [`Engine`] holds the open document and prepares its frames in the
//! background. It is shared by every thread: the interface thread asks for
//! frames and never waits; worker threads run [`Engine::work`].
//!
//! - [`Engine::open`] publishes a new document **generation**. Work in progress
//!   for an older generation finishes and is dropped.
//! - [`Engine::request`] replaces the list of wanted ticks (nearest first). It is
//!   how playback and scrubbing prefetch, and how obsolete work is cancelled: a
//!   tick that is no longer wanted and not yet started is never prepared.
//! - [`Engine::frame`] returns a prepared frame if one is ready.
//!
//! Nothing here blocks the calling thread except [`Engine::work`]. The browser's
//! interface thread may not wait on a lock, so every shared structure is taken
//! with [`sync::Shared::lock`], which spins for the few instructions a holder needs.

pub mod edit;
mod frames;
mod images;
pub mod mirror;
mod sync;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use styk_eval::assets::Assets;
use styk_eval::eval::Prepared;

pub use frames::{FrameData, FrameStats};
pub use images::DecodedImage;
use sync::Shared;

/// A monotonic clock in milliseconds. The engine cannot read one itself on every
/// platform (WebAssembly has none), so the host supplies it.
pub type Clock = fn() -> f64;

/// Memory the frame cache may hold, by device class. Draw lists are small (tens
/// to hundreds of KiB), so even the smallest budget holds seconds of animation.
#[derive(Clone, Copy, Debug)]
pub struct Budgets {
    pub frame_cache_bytes: usize,
}

impl Budgets {
    /// `memory_gib` is the host's device-memory hint (`navigator.deviceMemory`), if any.
    pub fn for_device(memory_gib: Option<f64>) -> Budgets {
        let frame_cache_bytes = match memory_gib {
            Some(gib) if gib <= 2.0 => 48 << 20,
            Some(gib) if gib <= 4.0 => 128 << 20,
            Some(_) => 384 << 20,
            None => 192 << 20,
        };
        Budgets { frame_cache_bytes }
    }
}

/// One open document, immutable. A new one replaces it on every change.
pub struct Generation {
    pub id: u64,
    /// Generations of one lineage are revisions of the same project: they share an
    /// image atlas, so an edit never uploads pictures again.
    pub lineage: u64,
    /// The interface's revision counter this document corresponds to.
    pub revision: u64,
    pub prepared: Arc<Prepared>,
    pub assets: Arc<Assets>,
    pub open_ms: f64,
}

/// What [`Engine::open`] reports.
#[derive(Clone, Debug)]
pub struct Opened {
    pub generation: u64,
    pub name: String,
    pub version: u32,
    pub canvas: [u32; 2],
    pub fps: f64,
    pub ticks: u32,
    pub figures: usize,
    pub keyframes: usize,
    pub images: usize,
    pub open_ms: f64,
    /// Features the file uses that the engine does not draw yet.
    pub unsupported: Vec<String>,
}

struct State {
    current: Option<Arc<Generation>>,
    frames: frames::Cache,
    images: images::Store,
    /// Background tasks that are not frames (image decoding), oldest first.
    tasks: std::collections::VecDeque<Box<dyn FnOnce(&Engine) + Send>>,
}

struct Inner {
    state: Shared<State>,
    /// Changes staged for the next commit.
    staged: Shared<Vec<mirror::Change>>,
    mirror: std::sync::Mutex<mirror::Mirror>,
    clock: Clock,
    next_generation: AtomicU64,
    stopping: AtomicBool,
    progress: std::sync::atomic::AtomicU32,
}

/// The engine. Cheap to clone; every clone is the same engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    pub fn new(clock: Clock, budgets: Budgets) -> Engine {
        Engine {
            inner: Arc::new(Inner {
                state: Shared::new(State {
                    current: None,
                    frames: frames::Cache::new(budgets.frame_cache_bytes),
                    images: images::Store::default(),
                    tasks: Default::default(),
                }),
                staged: Shared::new(Vec::new()),
                mirror: Default::default(),
                clock,
                next_generation: AtomicU64::new(1),
                stopping: AtomicBool::new(false),
                progress: std::sync::atomic::AtomicU32::new(0),
            }),
        }
    }

    pub fn now(&self) -> f64 {
        (self.inner.clock)()
    }

    /// Decodes a file and publishes it as the open document. Runs on the calling
    /// thread: call it from a worker for anything but small files.
    pub fn open(&self, bytes: &[u8], extension: &str, name: &str) -> Result<Opened, styk_eval::Error> {
        let started = self.now();
        let document = styk_eval::open_bytes(bytes, extension, name)?;
        let prepared = Arc::new(Prepared::new(Arc::new(document)));
        let assets = Arc::new(Assets::build(&prepared.doc));
        let open_ms = self.now() - started;
        let lineage = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        let doc = prepared.doc.clone();
        let ticks = prepared.total_ticks;
        let id = self.publish(Generation { id: 0, lineage, revision: 0, prepared, assets, open_ms });
        Ok(Opened {
            generation: id,
            name: doc.name.clone(),
            version: doc.version,
            canvas: doc.canvas,
            fps: doc.fps,
            ticks,
            figures: doc.figures.len(),
            keyframes: doc.keyframes.len(),
            images: doc.images.len(),
            open_ms,
            unsupported: doc.unsupported.clone(),
        })
    }

    /// Makes `generation` the open document and queues the decoding of its images
    /// that the previous revision did not already have. Returns its id.
    pub(crate) fn publish(&self, mut generation: Generation) -> u64 {
        let id = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        generation.id = id;
        let generation = Arc::new(generation);
        let image_count = generation.prepared.doc.images.len();
        {
            let mut state = self.inner.state.lock();
            let known = match &state.current {
                Some(previous) if previous.lineage == generation.lineage => previous.prepared.doc.images.len().min(image_count),
                _ => 0,
            };
            state.frames.reset(id);
            if known == 0 {
                state.images.reset(generation.lineage, image_count);
                state.tasks.clear();
            } else {
                state.images.grow(image_count - known);
            }
            // Image pixels are decoded in the background: the frames drawn before they
            // arrive show the artwork without them.
            for index in known..image_count {
                let generation = generation.clone();
                state.tasks.push_back(Box::new(move |engine: &Engine| engine.decode_image(&generation, index)));
            }
            state.current = Some(generation);
        }
        self.inner.state.notify_all();
        id
    }

    /// Runs `task` in the background: on a worker inside [`Engine::work`], or on the
    /// next [`Engine::step`] of a host without workers. Tasks run before frames.
    pub fn spawn(&self, task: impl FnOnce(&Engine) + Send + 'static) {
        self.inner.state.lock().tasks.push_back(Box::new(task));
        self.inner.state.notify_all();
    }

    /// A counter that advances whenever background work finishes (a frame, an image,
    /// a task). A host that cannot be called back watches it instead of polling results.
    pub fn progress(&self) -> &std::sync::atomic::AtomicU32 {
        &self.inner.progress
    }

    /// The open document, if any.
    pub fn generation(&self) -> Option<Arc<Generation>> {
        self.inner.state.lock().current.clone()
    }

    /// Asks every worker to return from [`Engine::work`].
    pub fn stop(&self) {
        self.inner.stopping.store(true, Ordering::SeqCst);
        self.inner.state.notify_all();
    }

    /// Runs background work on the calling thread until [`Engine::stop`]. Blocks while
    /// there is nothing to do, so it must not be called on a browser's interface thread.
    pub fn work(&self) {
        loop {
            if self.inner.stopping.load(Ordering::SeqCst) {
                return;
            }
            if !self.step() {
                self.inner.state.wait_for_work(&self.inner.stopping);
            }
        }
    }

    /// Runs one unit of background work on the calling thread, if there is any.
    /// A host without worker threads calls this between its own tasks.
    pub fn step(&self) -> bool {
        let job = {
            let mut state = self.inner.state.lock();
            if let Some(task) = state.tasks.pop_front() {
                Job::Task(task)
            } else {
                match (state.current.clone(), state.frames.take_wanted()) {
                    (Some(generation), Some(tick)) => Job::Frame(generation, tick),
                    _ => return false,
                }
            }
        };
        match job {
            Job::Task(task) => task(self),
            Job::Frame(generation, tick) => {
                let data = frames::produce(&generation, tick, self.inner.clock);
                self.inner.state.lock().frames.finish(generation.id, tick, Some(data));
            }
        }
        self.inner.progress.fetch_add(1, Ordering::SeqCst);
        true
    }
}

enum Job {
    Task(Box<dyn FnOnce(&Engine) + Send>),
    Frame(Arc<Generation>, u32),
}
