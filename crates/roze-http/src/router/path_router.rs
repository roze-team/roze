use std::{collections::BTreeMap, convert::Infallible, sync::Arc};

use http::{request::Parts, Method};
use matchit::Router as MatchRouter;
use tower::{util::BoxCloneSyncService, Layer, Service};

use crate::{
    extract::MatchedPath,
    rest::{HttpResponse, IncomingRequest},
    route_params::RouteParams,
};

use super::{
    method_not_allowed::{AllowHeader, MethodNotAllowed},
    path::join_paths,
    route::{boxed_service, layer_service, Route},
    strip_prefix::StripPrefixService,
    MethodRouter,
};

#[derive(Clone)]
pub(super) struct PathRouter {
    pub(super) routes: MatchRouter<usize>,
    pub(super) route_groups: Vec<RouteGroup>,
    pub(super) path_index: BTreeMap<String, usize>,
}

impl Default for PathRouter {
    fn default() -> Self {
        Self {
            routes: MatchRouter::new(),
            route_groups: Vec::new(),
            path_index: BTreeMap::new(),
        }
    }
}

impl PathRouter {
    pub(super) fn nest(
        &mut self,
        prefix: String,
        router: Self,
        default_method_not_allowed_fallback: Option<
            BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        >,
    ) {
        let Self {
            path_index,
            route_groups,
            ..
        } = router;
        for (path, group_index) in path_index {
            let nested_path = join_paths(&prefix, &path);
            let mut group = route_groups[group_index].clone();
            for route in &mut group.routes {
                route.service = boxed_service(StripPrefixService::new(
                    prefix.clone(),
                    route.service.clone(),
                ));
            }
            if let Some(fallback) = group.method_not_allowed_fallback.take() {
                group.method_not_allowed_fallback = Some(boxed_service(StripPrefixService::new(
                    prefix.clone(),
                    fallback,
                )));
            }
            self.insert_group_routes(
                nested_path,
                group,
                default_method_not_allowed_fallback.clone(),
                "nested",
            );
        }
    }

    pub(super) fn merge(
        &mut self,
        router: Self,
        default_method_not_allowed_fallback: Option<
            BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        >,
    ) {
        for (path, group_index) in router.path_index {
            let group = router.route_groups[group_index].clone();
            self.insert_group_routes(
                path,
                group,
                default_method_not_allowed_fallback.clone(),
                "merged",
            );
        }
    }

    #[track_caller]
    pub(super) fn route(
        &mut self,
        path: String,
        method_router: MethodRouter,
        default_method_not_allowed_fallback: Option<
            BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        >,
    ) {
        let group_index = self.ensure_route_group(path, default_method_not_allowed_fallback);
        let group = &mut self.route_groups[group_index];

        for endpoint in method_router.endpoints {
            if group
                .routes
                .iter()
                .any(|route| routes_overlap(&route.method, &endpoint.method))
            {
                panic!(
                    "overlapping method route for {}",
                    method_label(&endpoint.method)
                );
            }
            group.routes.push(Route {
                method: endpoint.method,
                service: endpoint.service,
            });
        }
        if let Some(fallback) = method_router.method_not_allowed_fallback {
            if group.method_not_allowed_fallback.is_some() {
                panic!("overlapping method-not-allowed fallback for {}", group.path);
            }
            group.method_not_allowed_fallback = Some(fallback);
        }
        group.refresh_allow_header();
    }

    pub(super) fn has_routes(&self) -> bool {
        !self.route_groups.is_empty()
    }

    pub(super) fn set_method_not_allowed_fallback(
        &mut self,
        fallback: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) {
        for group in &mut self.route_groups {
            group.method_not_allowed_fallback = Some(fallback.clone());
        }
    }

    pub(super) fn layer_routes<L>(&mut self, layer: L, include_method_fallbacks: bool)
    where
        L: Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
            + Clone
            + Send
            + Sync
            + 'static,
        L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as Service<IncomingRequest>>::Future: Send + 'static,
    {
        for group in &mut self.route_groups {
            for route in &mut group.routes {
                route.service = layer_service(layer.clone(), route.service.clone());
            }
            if include_method_fallbacks {
                if let Some(fallback) = group.method_not_allowed_fallback.take() {
                    group.method_not_allowed_fallback =
                        Some(layer_service(layer.clone(), fallback));
                }
            }
        }
    }

    pub(super) fn match_service(
        &self,
        parts: &mut Parts,
    ) -> Option<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>> {
        let Some(matched) = self.routes.at(parts.uri.path()).ok() else {
            parts.extensions.remove::<MatchedPath>();
            parts.extensions.remove::<RouteParams>();
            return None;
        };

        let route_params = RouteParams::from_pairs(
            matched
                .params
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        );
        parts.extensions.insert(route_params);

        let group = &self.route_groups[*matched.value];
        parts
            .extensions
            .insert(MatchedPath::new(group.path.clone()));
        let route = group
            .routes
            .iter()
            .find(|route| route.method.as_ref() == Some(&parts.method))
            .or_else(|| {
                (parts.method == Method::HEAD).then(|| {
                    group
                        .routes
                        .iter()
                        .find(|route| route.method.as_ref() == Some(&Method::GET))
                })?
            })
            .or_else(|| group.routes.iter().find(|route| route.method.is_none()));

        Some(route.map(|route| route.service.clone()).unwrap_or_else(|| {
            group
                .method_not_allowed_fallback
                .clone()
                .unwrap_or_else(|| boxed_service(MethodNotAllowed::new(group.allow_header.clone())))
        }))
    }

    pub(super) fn ensure_route_group(
        &mut self,
        path: String,
        method_not_allowed_fallback: Option<
            BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        >,
    ) -> usize {
        match self.path_index.get(&path).copied() {
            Some(index) => index,
            None => {
                let index = self.route_groups.len();
                self.routes
                    .insert(path.clone(), index)
                    .expect("invalid route path");
                self.route_groups.push(RouteGroup {
                    path: Arc::from(path.as_str()),
                    routes: Vec::new(),
                    allow_header: AllowHeader::default(),
                    method_not_allowed_fallback,
                });
                self.path_index.insert(path, index);
                index
            }
        }
    }

    pub(super) fn insert_group_routes(
        &mut self,
        path: String,
        group: RouteGroup,
        default_method_not_allowed_fallback: Option<
            BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        >,
        action: &str,
    ) {
        let RouteGroup {
            routes,
            method_not_allowed_fallback,
            ..
        } = group;
        let group_index = self.ensure_route_group(path, default_method_not_allowed_fallback);
        for route in routes {
            if self.route_groups[group_index]
                .routes
                .iter()
                .any(|existing| routes_overlap(&existing.method, &route.method))
            {
                panic!(
                    "overlapping {action} route for {}",
                    method_label(&route.method)
                );
            }
            self.route_groups[group_index].routes.push(route);
        }
        if let Some(fallback) = method_not_allowed_fallback {
            if self.route_groups[group_index]
                .method_not_allowed_fallback
                .is_some()
            {
                panic!("overlapping {action} method-not-allowed fallback");
            }
            self.route_groups[group_index].method_not_allowed_fallback = Some(fallback);
        }
        self.route_groups[group_index].refresh_allow_header();
    }
}

#[derive(Clone)]
pub(super) struct RouteGroup {
    pub(super) path: Arc<str>,
    pub(super) routes: Vec<Route>,
    pub(super) allow_header: AllowHeader,
    pub(super) method_not_allowed_fallback:
        Option<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>,
}

impl RouteGroup {
    pub(super) fn refresh_allow_header(&mut self) {
        self.allow_header =
            AllowHeader::from_methods(self.routes.iter().map(|route| route.method.as_ref()));
    }
}

pub(super) fn routes_overlap(left: &Option<Method>, right: &Option<Method>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

pub(super) fn method_label(method: &Option<Method>) -> String {
    method
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "any".to_string())
}
