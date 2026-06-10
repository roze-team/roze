use sea_orm::{DatabaseConnection, DbErr, EntityTrait, FromQueryResult, PaginatorTrait, Select};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    pub page: u64,
    pub page_size: u64,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
        }
    }
}

impl PageRequest {
    pub fn new(page: u64, page_size: u64) -> Self {
        Self {
            page: page.max(1),
            page_size: page_size.max(1),
        }
    }

    pub fn offset(self) -> u64 {
        self.page.saturating_sub(1) * self.page_size
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: PageRequest,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: u64, page: PageRequest) -> Self {
        Self { items, total, page }
    }

    pub fn empty(page: PageRequest) -> Self {
        Self {
            items: Vec::new(),
            total: 0,
            page,
        }
    }

    pub fn map<U, F>(self, mut f: F) -> Page<U>
    where
        F: FnMut(T) -> U,
    {
        Page {
            items: self.items.into_iter().map(&mut f).collect(),
            total: self.total,
            page: self.page,
        }
    }
}

#[derive(Debug, Error)]
pub enum OrmError {
    #[error("database error: {0}")]
    Database(#[from] DbErr),
    #[error("invalid page size")]
    InvalidPageSize,
}

pub async fn paginate_select<E>(
    db: &DatabaseConnection,
    select: Select<E>,
    page: PageRequest,
) -> Result<Page<E::Model>, OrmError>
where
    E: EntityTrait,
    E::Model: FromQueryResult + Send + Sync + 'static,
{
    if page.page_size == 0 {
        return Err(OrmError::InvalidPageSize);
    }

    let paginator = select.paginate(db, page.page_size);
    let total = paginator.num_items().await?;
    let items = paginator.fetch_page(page.page.saturating_sub(1)).await?;
    Ok(Page::new(items, total, page))
}

pub trait Repository {
    type Model;

    fn page_request(&self) -> PageRequest {
        PageRequest::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_request_clamps_to_valid_ranges() {
        let request = PageRequest::new(0, 0);
        assert_eq!(request.page, 1);
        assert_eq!(request.page_size, 1);
        assert_eq!(request.offset(), 0);
    }

    #[test]
    fn page_maps_items() {
        let page = Page::new(vec![1, 2, 3], 3, PageRequest::default());
        let mapped = page.map(|value| value.to_string());
        assert_eq!(mapped.items, vec!["1", "2", "3"]);
    }
}
