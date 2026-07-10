use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RouteParams(Arc<RouteParamsInner>);

#[derive(Debug, PartialEq, Eq)]
enum RouteParamsInner {
    Params(Vec<(String, String)>),
    InvalidUtf8(String),
}

impl Default for RouteParams {
    fn default() -> Self {
        Self(Arc::new(RouteParamsInner::Params(Vec::new())))
    }
}

impl RouteParams {
    pub(crate) fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let params = pairs
            .into_iter()
            .map(|(key, value)| {
                let key = percent_encoding::percent_decode_str(&key)
                    .decode_utf8()
                    .map_err(|error| error.to_string())?;
                let value = percent_encoding::percent_decode_str(&value)
                    .decode_utf8()
                    .map_err(|error| error.to_string())?;
                Ok((key.into_owned(), value.into_owned()))
            })
            .collect::<Result<Vec<_>, _>>();

        match params {
            Ok(params) => Self(Arc::new(RouteParamsInner::Params(params))),
            Err(error) => Self(Arc::new(RouteParamsInner::InvalidUtf8(error))),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), roze_error::RozeError> {
        match self.0.as_ref() {
            RouteParamsInner::Params(_) => Ok(()),
            RouteParamsInner::InvalidUtf8(error) => {
                Err(roze_error::RozeError::BadRequest(error.clone()))
            }
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.params()
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params()
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.params().is_empty()
    }

    fn params(&self) -> &[(String, String)] {
        match self.0.as_ref() {
            RouteParamsInner::Params(params) => params,
            RouteParamsInner::InvalidUtf8(_) => &[],
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
