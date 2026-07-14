pub mod backends;
pub mod registry;

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use backends::Backend;
use registry::Registry;

/// An xkb layout selection: layout code plus optional variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSpec {
    pub layout: String,
    pub variant: Option<String>,
}

impl LayoutSpec {
    pub fn new(layout: impl Into<String>, variant: Option<impl Into<String>>) -> Self {
        Self {
            layout: layout.into(),
            variant: variant.map(Into::into),
        }
    }

    /// Parse a config-file value: "pt" or "pt(nativo)".
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some((layout, rest)) = s.split_once('(') {
            let variant = rest.trim_end_matches(')').trim();
            if !variant.is_empty() {
                return Self::new(layout.trim(), Some(variant));
            }
        }
        Self::new(s, None::<String>)
    }
}

impl std::fmt::Display for LayoutSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.variant {
            Some(v) => write!(f, "{}({})", self.layout, v),
            None => write!(f, "{}", self.layout),
        }
    }
}

const POLL_INTERVAL: Duration = Duration::from_secs(3);

pub struct Detection {
    pub spec: LayoutSpec,
    /// None when the config pinned the layout — no watcher then.
    pub backend: Option<Backend>,
    pub registry: Arc<Registry>,
}

/// Resolve the layout to start with. Config override wins and disables
/// detection entirely (spec: the universal escape hatch).
pub fn init(config_layout: Option<&str>) -> Detection {
    if let Some(value) = config_layout {
        let spec = LayoutSpec::parse(value);
        eprintln!("kbcut: layout '{spec}' pinned by config, detection disabled");
        return Detection {
            spec,
            backend: None,
            registry: Arc::new(Registry::from_xml("")),
        };
    }
    let registry = Arc::new(Registry::load());
    let backend = backends::select_backend(|k| std::env::var(k).ok());
    let (spec, backend) = match backend {
        Some(b) => match b.current(&registry) {
            Ok(spec) => {
                eprintln!("kbcut: layout '{spec}' via {} backend", b.name());
                (spec, Some(b))
            }
            Err(e) => {
                eprintln!(
                    "kbcut: {} layout detection failed ({e:#}); using 'us'. \
                     Set `layout = \"...\"` in the config to override.",
                    b.name()
                );
                (LayoutSpec::new("us", None::<String>), Some(b))
            }
        },
        None => (LayoutSpec::new("us", None::<String>), None),
    };
    Detection {
        spec,
        backend,
        registry,
    }
}

/// Watch for live layout switches. Calls `on_change` with each NEW layout
/// (deduplicated). Event stream when the backend has one; if the stream dies
/// or was never available, poll every POLL_INTERVAL (spec fallback rule).
pub fn spawn_watcher(
    backend: Backend,
    initial: LayoutSpec,
    registry: Arc<Registry>,
    on_change: impl Fn(LayoutSpec) + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut last = initial;
        if let Some(stream) = backend.watch_stream() {
            for line in stream.lines() {
                if line.is_err() {
                    break; // child/socket died → fall through to polling
                }
                redetect(backend, &registry, &mut last, &on_change);
            }
            eprintln!("kbcut: layout event stream ended, falling back to polling");
        }
        loop {
            std::thread::sleep(POLL_INTERVAL);
            redetect(backend, &registry, &mut last, &on_change);
        }
    });
}

fn redetect(
    backend: Backend,
    registry: &Registry,
    last: &mut LayoutSpec,
    on_change: &impl Fn(LayoutSpec),
) {
    // Detection failure keeps the last good keymap (spec error philosophy).
    if let Ok(spec) = backend.current(registry) {
        if spec != *last {
            *last = spec.clone();
            on_change(spec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_layout_values() {
        assert_eq!(
            LayoutSpec::parse("pt"),
            LayoutSpec::new("pt", None::<String>)
        );
        assert_eq!(
            LayoutSpec::parse("pt(nativo)"),
            LayoutSpec::new("pt", Some("nativo"))
        );
        assert_eq!(
            LayoutSpec::parse(" us "),
            LayoutSpec::new("us", None::<String>)
        );
    }
}
