use std::{
    path::{
        Path,
        PathBuf,
    },
    rc::Rc,
    sync::{
        atomic::{
            AtomicBool,
            Ordering,
        },
        Arc,
    },
    time::{
        Duration,
        Instant,
    },
    thread,
};

use glium::backend::Context;

use notify::{
    RecommendedWatcher,
    RecursiveMode,
    Watcher,
};

use signal_hook::{consts::SIGUSR1, iterator::Signals};

use crate::{
    graph::ShaderGraph,
    lisp::graph_from_sexp,
    map,
    reload::ShaderDir,
};

/// A struct that watches a directory for changes,
/// and hot-reloads a shader graph if changes have been
/// made.
pub struct ShaderGraphWatcher {
    context:      Rc<Context>,
    last_reload:  Instant,
    path:         PathBuf,
    config:       PathBuf,
    changed:      Arc<AtomicBool>,
    _watcher:     RecommendedWatcher,
    shader_graph: ShaderGraph,
}

pub enum WatchResult {
    /// No changes were made.
    NoChange,
    /// Changes were made and the graph was rebuilt.
    Rebuilt,
    /// Changes were made but the graph could not be
    /// rebuilt.
    Err(String),
}

impl ShaderGraphWatcher {
    /// Creates a new watcher over a certain dir.
    /// Returns an error if the directory could not be
    /// loaded, Or the graph could not be built.
    pub fn new_watch_dir<T>(
        context: &Rc<Context>,
        path: T,
        config: T,
    ) -> Result<ShaderGraphWatcher, String>
    where
        T: AsRef<Path>,
    {
        let path = path.as_ref().to_path_buf();
        let config = config.as_ref().to_path_buf();

        let changed = Arc::new(AtomicBool::new(false));
        // build the watcher
        let mut watcher = RecommendedWatcher::new({
            let changed = changed.clone();
            move |res| match res {
                Ok(_) => changed.store(true, Ordering::SeqCst),
                Err(e) => println!("[warn] Watch error: `{:?}`.", e),
            }
        })
        .unwrap();
        watcher.watch(&path, RecursiveMode::Recursive).unwrap();

        let signals = Signals::new(&[SIGUSR1]);
        match signals {
            Ok(mut s) => {
                    let changed = changed.clone();
                    thread::spawn(move || {
                        for sig in s.forever() {
                            changed.store(true, Ordering::SeqCst);
                            println!("[info] Received signal {:?}", sig);
                        }
                    });
                }
            Err(e) => println!("[warn] Signal listen error: `{:?}`.", e)
        };

        let shader_graph = ShaderGraphWatcher::build(context, &path, &config)?;
        let last_reload = Instant::now();

        Ok(ShaderGraphWatcher {
            context: context.clone(),
            last_reload,
            path,
            config,
            changed,
            _watcher: watcher,
            shader_graph,
        })
    }

    fn build(
        context: &Rc<Context>,
        path: &Path,
        config: &Path,
    ) -> Result<ShaderGraph, String> {
        let shader_dir = ShaderDir::new_from_dir(path, config)?;
        let shader_graph = graph_from_sexp(context, shader_dir, map! {})?;
        Ok(shader_graph)
    }

    /// Gets the shader graph without trying to reload
    /// Note that `graph` will only reload when needed,
    /// And tries to de-duplicate redundant reloads,
    /// So only use this for fine-grained control over
    /// reloads.
    pub fn graph_no_reload(&mut self) -> &mut ShaderGraph {
        &mut self.shader_graph
    }

    /// Forces a rebuild of the graph. Do not call this in a
    /// loop! As with `graph_no_reload`, only use this
    /// for fine-grained control over reloads.
    pub fn graph_force_reload(&mut self) -> (&mut ShaderGraph, WatchResult) {
        let watch_result = match ShaderGraphWatcher::build(
            &self.context,
            &self.path,
            &self.config,
        ) {
            Ok(graph) => {
                self.shader_graph = graph;
                WatchResult::Rebuilt
            },
            Err(error) => WatchResult::Err(error),
        };

        self.last_reload = Instant::now();
        (&mut self.shader_graph, watch_result)
    }

    /// Reloads a shader graph if there have been changes,
    /// And the graph hasn't been rebuilt recently.
    /// Note that if compilation fails, the old graph will
    /// remain in use. Returns a borrowed `ShaderGraph`,
    /// and whether the graph was rebuilt.
    pub fn graph(&mut self) -> (&mut ShaderGraph, WatchResult) {
        if self.last_reload.elapsed() > Duration::from_millis(300)
            && self.changed.swap(false, Ordering::SeqCst)
        {
            self.graph_force_reload()
        } else {
            (self.graph_no_reload(), WatchResult::NoChange)
        }
    }
}
