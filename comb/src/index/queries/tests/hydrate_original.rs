pub(in crate::index) const HYDRATE_GRAPH: &str = "SELECT title,kind,substr(compiled,1,?2),'','compiled',verified,verified FROM pages WHERE slug=?1";
