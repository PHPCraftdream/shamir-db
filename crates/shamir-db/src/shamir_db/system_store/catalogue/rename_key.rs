/// Identity of one table rename in the catalogue.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TableRenameKey<'a> {
    pub(crate) db_name: &'a str,
    pub(crate) repo_name: &'a str,
    pub(crate) from: &'a str,
    pub(crate) to: &'a str,
}
