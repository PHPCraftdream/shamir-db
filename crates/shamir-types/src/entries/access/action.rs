use std::fmt;

/// The class of operation being performed on a resource.
///
/// POSIX-flavoured: `Read`/`Write`/`Execute` map to `r`/`w`/`x`; `Create`
/// and `Delete` are writes on a container; `List` is read on a container;
/// `Manage` is the owner/admin-only class (chmod/chown/chgrp/grant).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Read,
    Write,
    Create,
    Delete,
    Execute,
    List,
    Manage,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "READ",
            Self::Write => "WRITE",
            Self::Create => "CREATE",
            Self::Delete => "DELETE",
            Self::Execute => "EXECUTE",
            Self::List => "LIST",
            Self::Manage => "MANAGE",
        })
    }
}
