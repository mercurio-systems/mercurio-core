pub mod api;
pub mod workspace;

use std::net::SocketAddr;
use std::path::Path;

pub use api::{
    ApiError, AppState, EditorDiagnosticDto, EditorFileContentDto, EditorFileListDto,
    EditorFileSummaryDto, EditorFormatResponseDto, EditorLintResponseDto, EditorOutlineNodeDto,
    EditorParseResponseDto, EditorRefreshResponseDto, EditorSemanticCompileResponseDto,
    ElementDetailsDto, ElementPropertyRowDto, ElementPropertyTableDto,
    EvaluateExpressionRequestDto, EvaluateExpressionResponseDto, ExplorerAttributeDto, GraphDto,
    GraphEdgeDto, GraphNodeDto, InheritedPropertyValueDto, L2ExplorerEdgeDto, L2ExplorerGraphDto,
    L2ExplorerNodeDto, L2ExplorerRequestDto, LibraryTreeNodeDto, MetatypeExplorerEdgeDto,
    MetatypeExplorerGraphDto, MetatypeExplorerNodeDto, MetatypeExplorerRequestDto,
    ModelMetadataDto, MountedLibraryTreeDto, ProjectLintFileResultDto, ProjectLintResponseDto,
    SearchResultDto, SemanticCompileFileResultDto, SemanticProjectCompileRequestDto,
    SemanticProjectCompileResponseDto, SemanticWorkspaceFileDto, SemanticWorkspaceSessionDto,
    ServerLoginRequestDto, ServerLoginResponseDto, ServerProjectCompileResponseDto, ServerState,
    ServerUserDto, StagedEditorFileDto, WorkspaceProjectInfoDto, WorkspaceProjectLibraryDto,
    build_router, load_app_state, load_server_state,
};
pub use workspace::{CompileProjectScopeOutcome, WorkspaceService, load_workspace_service};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStartupInfo {
    pub url: String,
    pub pid: u32,
    pub version: String,
    pub api_version: u32,
}

pub async fn serve_workspace_http(
    host: &str,
    port: u16,
    workspace_root: &Path,
) -> Result<ServerStartupInfo, Box<dyn std::error::Error + Send + Sync>> {
    let state = WorkspaceService::from_workspace_root_shell(workspace_root)?;
    let app = build_router(api::ServerState::new(state));
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    let local_addr = listener.local_addr()?;
    let startup = startup_info(local_addr);
    axum::serve(listener, app).await?;
    Ok(startup)
}

pub async fn serve_workspace_http_with_startup<F>(
    host: &str,
    port: u16,
    workspace_root: &Path,
    on_startup: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce(&ServerStartupInfo),
{
    let state = WorkspaceService::from_workspace_root_shell(workspace_root)?;
    let app = build_router(api::ServerState::new(state));
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    let startup = startup_info(listener.local_addr()?);
    on_startup(&startup);
    axum::serve(listener, app).await?;
    Ok(())
}

fn startup_info(addr: SocketAddr) -> ServerStartupInfo {
    ServerStartupInfo {
        url: format!("http://{addr}"),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: 1,
    }
}
