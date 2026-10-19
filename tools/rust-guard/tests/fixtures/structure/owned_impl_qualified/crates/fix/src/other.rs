// An inherent impl of `RepoInstance` OUTSIDE its owner module, written
// with a fully-qualified self type: the governed name is the LAST path
// segment (finding 10d).
impl crate::repo::instance::RepoInstance {
    pub fn outside_owner(&self) -> u32 {
        2
    }
}
