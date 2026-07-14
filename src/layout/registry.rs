//! Maps descriptive layout names ("Portuguese (Nativo)") to xkb codes
//! ("pt", Some("nativo")) by parsing the xkb registry, evdev.xml.
//! Needed because Sway and KDE report descriptions, not codes.

use std::collections::HashMap;

use super::LayoutSpec;

const EVDEV_XML_PATHS: &[&str] = &[
    "/usr/share/X11/xkb/rules/evdev.xml",
    "/usr/share/xkeyboard-config-2/rules/evdev.xml", // some distros
];

pub struct Registry {
    by_description: HashMap<String, LayoutSpec>,
}

impl Registry {
    pub fn load() -> Self {
        for path in EVDEV_XML_PATHS {
            if let Ok(xml) = std::fs::read_to_string(path) {
                return Self::from_xml(&xml);
            }
        }
        eprintln!(
            "kbcut: xkb registry (evdev.xml) not found; descriptive layout names won't resolve"
        );
        Self {
            by_description: HashMap::new(),
        }
    }

    pub fn from_xml(xml: &str) -> Self {
        Self {
            by_description: parse_evdev_xml(xml),
        }
    }

    /// Resolve either an xkb code ("pt") or a description ("Portuguese").
    pub fn resolve(&self, name: &str) -> Option<LayoutSpec> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if let Some(spec) = self.by_description.get(name) {
            return Some(spec.clone());
        }
        // Already a code (short, ascii-lowercase, possibly "pt+nativo")
        if name.len() <= 32
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '_')
        {
            return Some(match name.split_once('+') {
                Some((l, v)) => LayoutSpec::new(l, Some(v)),
                None => LayoutSpec::new(name, None::<String>),
            });
        }
        None
    }
}

/// Line-oriented parse of evdev.xml. Only the <layoutList> section matters;
/// <modelList>/<optionList> also contain <name>/<description> pairs and must
/// be ignored. Structure inside layoutList:
///   <layout><configItem><name>pt</name>…<description>Portuguese</description>…
///     <variantList><variant><configItem><name>nativo</name>
///       <description>Portuguese (Nativo)</description>…
fn parse_evdev_xml(xml: &str) -> HashMap<String, LayoutSpec> {
    let mut map = HashMap::new();
    let mut in_layout_list = false;
    let mut in_variant = false;
    let mut current_layout: Option<String> = None;
    let mut pending_name: Option<String> = None;

    for line in xml.lines() {
        let line = line.trim();
        if line.starts_with("<layoutList") {
            in_layout_list = true;
        } else if line.starts_with("</layoutList") {
            break;
        }
        if !in_layout_list {
            continue;
        }
        if line.starts_with("<layout>") {
            in_variant = false;
            pending_name = None;
        } else if line.starts_with("<variant") {
            in_variant = true;
            pending_name = None;
        } else if let Some(name) = tag_content(line, "name") {
            if pending_name.is_none() {
                pending_name = Some(name.to_string());
            }
        } else if let Some(desc) = tag_content(line, "description") {
            if let Some(name) = pending_name.take() {
                if in_variant {
                    if let Some(layout) = &current_layout {
                        map.insert(
                            desc.to_string(),
                            LayoutSpec::new(layout.clone(), Some(name)),
                        );
                    }
                } else {
                    current_layout = Some(name.clone());
                    map.insert(desc.to_string(), LayoutSpec::new(name, None::<String>));
                }
            }
        }
    }
    map
}

fn tag_content<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = line.find(&open)? + open.len();
    let end = line.find(&close)?;
    (start <= end).then(|| &line[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
<xkbConfigRegistry>
  <modelList>
    <model><configItem><name>pc105</name><description>Generic 105-key PC</description></configItem></model>
  </modelList>
  <layoutList>
    <layout>
      <configItem>
        <name>us</name>
        <shortDescription>en</shortDescription>
        <description>English (US)</description>
      </configItem>
      <variantList>
        <variant>
          <configItem>
            <name>intl</name>
            <description>English (US, intl., with dead keys)</description>
          </configItem>
        </variant>
      </variantList>
    </layout>
    <layout>
      <configItem>
        <name>pt</name>
        <shortDescription>pt</shortDescription>
        <description>Portuguese</description>
      </configItem>
      <variantList>
        <variant>
          <configItem>
            <name>nativo</name>
            <description>Portuguese (Nativo)</description>
          </configItem>
        </variant>
      </variantList>
    </layout>
  </layoutList>
  <optionList>
    <group><configItem><name>grp</name><description>Switching to another layout</description></configItem></group>
  </optionList>
</xkbConfigRegistry>
"#;

    #[test]
    fn resolves_layout_descriptions() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(
            r.resolve("Portuguese"),
            Some(LayoutSpec::new("pt", None::<String>))
        );
        assert_eq!(
            r.resolve("English (US)"),
            Some(LayoutSpec::new("us", None::<String>))
        );
    }

    #[test]
    fn resolves_variant_descriptions() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(
            r.resolve("Portuguese (Nativo)"),
            Some(LayoutSpec::new("pt", Some("nativo")))
        );
    }

    #[test]
    fn passes_codes_through_and_splits_plus() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(r.resolve("pt"), Some(LayoutSpec::new("pt", None::<String>)));
        assert_eq!(
            r.resolve("pt+nativo"),
            Some(LayoutSpec::new("pt", Some("nativo")))
        );
    }

    #[test]
    fn ignores_models_and_options() {
        let r = Registry::from_xml(FIXTURE);
        assert_eq!(r.resolve("Generic 105-key PC"), None);
        assert_eq!(r.resolve("Switching to another layout"), None);
    }

    #[test]
    fn real_system_registry_parses_if_present() {
        // Smoke test against the actual file where available (dev machines, CI).
        if std::path::Path::new("/usr/share/X11/xkb/rules/evdev.xml").exists() {
            let r = Registry::load();
            assert_eq!(
                r.resolve("Portuguese"),
                Some(LayoutSpec::new("pt", None::<String>))
            );
        }
    }
}
