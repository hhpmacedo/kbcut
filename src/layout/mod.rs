pub mod registry;

/// An xkb layout selection: layout code plus optional variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSpec {
    pub layout: String,
    pub variant: Option<String>,
}

impl LayoutSpec {
    pub fn new(layout: impl Into<String>, variant: Option<impl Into<String>>) -> Self {
        Self { layout: layout.into(), variant: variant.map(Into::into) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_layout_values() {
        assert_eq!(LayoutSpec::parse("pt"), LayoutSpec::new("pt", None::<String>));
        assert_eq!(LayoutSpec::parse("pt(nativo)"), LayoutSpec::new("pt", Some("nativo")));
        assert_eq!(LayoutSpec::parse(" us "), LayoutSpec::new("us", None::<String>));
    }
}
