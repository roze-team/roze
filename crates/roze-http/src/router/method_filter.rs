use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not, Sub, SubAssign};

use http::Method;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MethodFilter(u16);

impl MethodFilter {
    pub const GET: Self = Self(1 << 0);
    pub const POST: Self = Self(1 << 1);
    pub const PUT: Self = Self(1 << 2);
    pub const PATCH: Self = Self(1 << 3);
    pub const DELETE: Self = Self(1 << 4);
    pub const HEAD: Self = Self(1 << 5);
    pub const OPTIONS: Self = Self(1 << 6);
    pub const TRACE: Self = Self(1 << 7);
    pub const CONNECT: Self = Self(1 << 8);
    pub const ALL: Self = Self(
        Self::GET.0
            | Self::POST.0
            | Self::PUT.0
            | Self::PATCH.0
            | Self::DELETE.0
            | Self::HEAD.0
            | Self::OPTIONS.0
            | Self::TRACE.0
            | Self::CONNECT.0,
    );

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn complement(self) -> Self {
        Self::ALL.without(self)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn from_method(method: &Method) -> Option<Self> {
        match *method {
            Method::GET => Some(Self::GET),
            Method::POST => Some(Self::POST),
            Method::PUT => Some(Self::PUT),
            Method::PATCH => Some(Self::PATCH),
            Method::DELETE => Some(Self::DELETE),
            Method::HEAD => Some(Self::HEAD),
            Method::OPTIONS => Some(Self::OPTIONS),
            Method::TRACE => Some(Self::TRACE),
            Method::CONNECT => Some(Self::CONNECT),
            _ => None,
        }
    }

    pub fn matches(self, method: &Method) -> bool {
        Self::from_method(method).is_some_and(|filter| self.contains(filter))
    }

    pub fn methods(self) -> Vec<Method> {
        [
            (Self::GET, Method::GET),
            (Self::POST, Method::POST),
            (Self::PUT, Method::PUT),
            (Self::PATCH, Method::PATCH),
            (Self::DELETE, Method::DELETE),
            (Self::HEAD, Method::HEAD),
            (Self::OPTIONS, Method::OPTIONS),
            (Self::TRACE, Method::TRACE),
            (Self::CONNECT, Method::CONNECT),
        ]
        .into_iter()
        .filter_map(|(filter, method)| self.contains(filter).then_some(method))
        .collect()
    }
}

impl BitOr for MethodFilter {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for MethodFilter {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for MethodFilter {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for MethodFilter {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Sub for MethodFilter {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        self.without(rhs)
    }
}

impl SubAssign for MethodFilter {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 &= !rhs.0;
    }
}

impl Not for MethodFilter {
    type Output = Self;

    fn not(self) -> Self::Output {
        self.complement()
    }
}

pub enum MethodSelection {
    Method(Method),
    Filter(MethodFilter),
}

impl MethodSelection {
    pub(super) fn methods(self) -> Vec<Method> {
        match self {
            Self::Method(method) => vec![method],
            Self::Filter(filter) => filter.methods(),
        }
    }
}

impl From<Method> for MethodSelection {
    fn from(method: Method) -> Self {
        Self::Method(method)
    }
}

impl From<MethodFilter> for MethodSelection {
    fn from(filter: MethodFilter) -> Self {
        Self::Filter(filter)
    }
}
