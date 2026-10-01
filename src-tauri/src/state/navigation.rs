use std::collections::VecDeque;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymlinkReturn {
    pub source_id: String,
    pub path: String,
    pub name: String,
    pub target_source_id: String,
    pub target_path: String,
}

/// Backend-only return routes, including snapshots for history Back/Forward.
#[derive(Clone, Debug, Default)]
pub struct NavigationContext {
    links: Vec<SymlinkReturn>,
    history: VecDeque<(String, String, Vec<SymlinkReturn>)>,
}

impl NavigationContext {
    pub fn enter_link(&mut self, link: SymlinkReturn) {
        self.record(&link.source_id, &link.path);
        self.links.push(link);
    }

    pub fn link_return(&self, source_id: &str, path: &str) -> Option<&SymlinkReturn> {
        self.links
            .last()
            .filter(|link| link.target_source_id == source_id && link.target_path == path)
    }

    pub fn leave_link(&mut self) {
        self.links.pop();
    }

    pub fn navigate(&mut self, source_id: &str, path: &str, in_shadow: bool) {
        if !in_shadow {
            while self.links.last().is_some_and(|link| {
                link.target_source_id != source_id
                    || !Path::new(path).starts_with(&link.target_path)
            }) {
                self.links.pop();
            }
        }
        self.record(source_id, path);
    }

    pub fn restore_history(&mut self, source_id: &str, path: &str) -> Option<String> {
        let cursor_hint = self
            .links
            .last()
            .filter(|link| link.source_id == source_id && link.path == path)
            .map(|link| link.name.clone());
        self.links = self
            .history
            .iter()
            .rev()
            .find(|(source, location, _)| source == source_id && location == path)
            .map(|(_, _, links)| links.clone())
            .unwrap_or_default();
        cursor_hint
    }

    fn record(&mut self, source_id: &str, path: &str) {
        self.history
            .retain(|(source, location, _)| source != source_id || location != path);
        self.history
            .push_back((source_id.into(), path.into(), self.links.clone()));
        // Keep return-context storage bounded, like the panel's navigation history.
        if self.history.len() > 50 {
            self.history.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(source: &str, origin: &str, target_source: &str, target: &str) -> SymlinkReturn {
        SymlinkReturn {
            source_id: source.into(),
            path: origin.into(),
            name: "link".into(),
            target_source_id: target_source.into(),
            target_path: target.into(),
        }
    }

    #[test]
    fn symlink_up_returns_only_at_target_boundary() {
        let mut context = NavigationContext::default();
        context.enter_link(link("local", "/origin", "local", "/target"));
        context.navigate("local", "/target/child", false);
        assert!(context.link_return("local", "/target/child").is_none());
        context.navigate("local", "/target", false);
        assert_eq!(
            context.link_return("local", "/target").unwrap().path,
            "/origin"
        );
        context.navigate("local", "/target-other", false);
        assert!(context.links.is_empty());
    }

    #[test]
    fn symlink_history_restores_cross_source_route_and_cursor() {
        let mut context = NavigationContext::default();
        context.enter_link(link("origin-volume", "/origin", "target-volume", "/target"));
        context.navigate("target-volume", "/target", false);
        assert_eq!(
            context.restore_history("origin-volume", "/origin"),
            Some("link".into())
        );
        context.navigate("origin-volume", "/origin", false);
        assert!(context.links.is_empty());
        context.restore_history("target-volume", "/target");
        context.navigate("target-volume", "/target", false);
        assert_eq!(
            context
                .link_return("target-volume", "/target")
                .unwrap()
                .source_id,
            "origin-volume"
        );
    }

    #[test]
    fn symlink_nested_links_return_in_order_even_outside_outer_target() {
        let mut context = NavigationContext::default();
        context.enter_link(link("local", "/origin", "local", "/target"));
        context.navigate("local", "/target/child", false);
        context.enter_link(link("local", "/target/child", "other", "/elsewhere"));
        context.navigate("other", "/elsewhere", false);
        assert_eq!(
            context.link_return("other", "/elsewhere").unwrap().path,
            "/target/child"
        );
        context.navigate("local", "/target/child", false);
        context.navigate("local", "/target", false);
        assert_eq!(
            context.link_return("local", "/target").unwrap().path,
            "/origin"
        );
    }

    #[test]
    fn symlink_archive_round_trip_preserves_return_route() {
        let mut context = NavigationContext::default();
        context.enter_link(link("local", "/origin", "local", "/target"));
        context.navigate("local", "/target", false);
        context.navigate("zip:/target/archive.zip", "", true);
        context.navigate("local", "/target", false);
        assert!(context.link_return("local", "/target").is_some());
    }
}
