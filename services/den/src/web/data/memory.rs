use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    core::memory_manager_head::{
        fetch_memfs_role_memory_file, fetch_memfs_role_memory_status, fetch_memfs_role_memory_tree,
        fetch_memfs_role_view_health, search_memfs_role_memory, MemfsRoleMemoryFileResponse,
        MemfsRoleMemorySearchResponse, MemfsRoleMemoryStatusResponse, MemfsRoleMemoryTreeResponse,
        MemfsViewHealth,
    },
    errors::CustomError,
};

/// MemFS/Memory Manager data required by Den's web UI.
#[async_trait]
pub trait WebMemoryDataSource: Send + Sync {
    fn is_configured(&self) -> bool;

    async fn fetch_role_view_health(
        &self,
        bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsViewHealth>, CustomError>;

    async fn fetch_role_memory_status(
        &self,
        bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsRoleMemoryStatusResponse>, CustomError>;

    async fn fetch_role_memory_tree(
        &self,
        bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsRoleMemoryTreeResponse>, CustomError>;

    async fn search_role_memory(
        &self,
        bear_id: Uuid,
        role: &str,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Option<MemfsRoleMemorySearchResponse>, CustomError>;

    async fn fetch_role_memory_file(
        &self,
        bear_id: Uuid,
        role: &str,
        path: &str,
    ) -> Result<Option<MemfsRoleMemoryFileResponse>, CustomError>;
}

#[derive(Clone)]
pub struct RealWebMemoryDataSource {
    http: reqwest::Client,
    memfs_base_url: String,
}

impl RealWebMemoryDataSource {
    pub fn new(http: reqwest::Client, memfs_base_url: String) -> Self {
        Self {
            http,
            memfs_base_url,
        }
    }
}

#[async_trait]
impl WebMemoryDataSource for RealWebMemoryDataSource {
    fn is_configured(&self) -> bool {
        !self.memfs_base_url.trim().is_empty()
    }

    async fn fetch_role_view_health(
        &self,
        bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsViewHealth>, CustomError> {
        fetch_memfs_role_view_health(&self.http, &self.memfs_base_url, bear_id, role).await
    }

    async fn fetch_role_memory_status(
        &self,
        bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsRoleMemoryStatusResponse>, CustomError> {
        fetch_memfs_role_memory_status(&self.http, &self.memfs_base_url, bear_id, role).await
    }

    async fn fetch_role_memory_tree(
        &self,
        bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsRoleMemoryTreeResponse>, CustomError> {
        fetch_memfs_role_memory_tree(&self.http, &self.memfs_base_url, bear_id, role).await
    }

    async fn search_role_memory(
        &self,
        bear_id: Uuid,
        role: &str,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Option<MemfsRoleMemorySearchResponse>, CustomError> {
        search_memfs_role_memory(
            &self.http,
            &self.memfs_base_url,
            bear_id,
            role,
            query,
            limit,
        )
        .await
    }

    async fn fetch_role_memory_file(
        &self,
        bear_id: Uuid,
        role: &str,
        path: &str,
    ) -> Result<Option<MemfsRoleMemoryFileResponse>, CustomError> {
        fetch_memfs_role_memory_file(&self.http, &self.memfs_base_url, bear_id, role, path).await
    }
}
