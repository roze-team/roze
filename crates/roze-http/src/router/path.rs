pub(super) fn normalize_path(path: String) -> String {
    if path.is_empty() {
        panic!("route path must not be empty");
    }
    if !path.starts_with('/') {
        panic!("route path must start with `/`");
    }
    validate_path_segments(&path);
    path
}

fn validate_path_segments(path: &str) {
    for segment in path.split('/') {
        if segment.starts_with(':') {
            panic!("route path segments must not start with `:`; use `{{param}}` captures");
        }
        if segment.starts_with('*') {
            panic!("route path segments must not start with `*`; use `{{*wildcard}}` captures");
        }
    }
}

pub(super) fn normalize_nest_prefix(prefix: String, root_hint: &str) -> String {
    let prefix = normalize_path(prefix);
    let prefix = prefix.trim_end_matches('/').to_string();
    if prefix.is_empty() || prefix == "/" {
        panic!("nest prefix must not be root; {root_hint}");
    }
    if prefix
        .split('/')
        .any(|segment| segment.starts_with("{*") && segment.ends_with('}'))
    {
        panic!("nest prefix must not contain wildcard captures");
    }
    prefix
}

pub(super) fn join_paths(prefix: &str, path: &str) -> String {
    if path == "/" {
        prefix.to_string()
    } else {
        format!("{prefix}{}", normalize_path(path.to_string()))
    }
}
