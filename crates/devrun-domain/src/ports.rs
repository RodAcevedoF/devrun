use std::path::Path;

use crate::RepoWorkspace;

pub trait WorkspaceRepository {
    type Error;

    fn load(&self, root: &Path) -> Result<Option<RepoWorkspace>, Self::Error>;

    fn save(&self, workspace: &RepoWorkspace) -> Result<(), Self::Error>;
}
