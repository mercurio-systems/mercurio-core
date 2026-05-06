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

pub async fn serve_mockup_http_with_startup<F>(
    host: &str,
    port: u16,
    workspace_root: &Path,
    static_dir: Option<&Path>,
    on_startup: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce(&ServerStartupInfo),
{
    ensure_mockup_workspace(workspace_root)?;
    if let Some(static_dir) = static_dir {
        unsafe {
            std::env::set_var("MERCURIO_STATIC_DIR", static_dir);
        }
    }
    unsafe {
        std::env::set_var("MERCURIO_MOCKUP_MODE", "1");
    }
    let state = WorkspaceService::from_workspace_root(workspace_root)?;
    let app = build_router(api::ServerState::new(state));
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    let startup = startup_info(listener.local_addr()?);
    on_startup(&startup);
    axum::serve(listener, app).await?;
    Ok(())
}

fn ensure_mockup_workspace(workspace_root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(workspace_root.join("scripts"))?;
    let has_existing_entries = std::fs::read_dir(workspace_root)?.next().is_some();
    if has_existing_entries && workspace_root.join("src").exists() {
        return Ok(());
    }

    let src_dir = workspace_root.join("src");
    std::fs::create_dir_all(&src_dir)?;
    let descriptor_path = workspace_root.join(mercurio_core::PROJECT_DESCRIPTOR_FILE_NAME);
    if !descriptor_path.exists() {
        std::fs::write(
            &descriptor_path,
            "{\n  \"version\": 1,\n  \"name\": \"Mockup Workspace\",\n  \"baseline_libraries\": [],\n  \"libraries\": []\n}\n",
        )?;
    }
    let model_path = src_dir.join("main.sysml");
    if !model_path.exists() {
        std::fs::write(
            model_path,
            "package Mockup {\n  part def System {\n    attribute mass = 42;\n  }\n\n  part system : System;\n}\n",
        )?;
    }
    let script_path = workspace_root.join("scripts").join("hello.py");
    if !script_path.exists() {
        std::fs::write(
            script_path,
            "from pathlib import Path\n\nprint('Mockup workspace files:')\nfor path in sorted(Path('.').glob('**/*')):\n    if path.is_file():\n        print(path)\n",
        )?;
    }
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
