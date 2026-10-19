// An inherent impl of RepoInstance outside its defining module tree:
impl RepoInstance {
    pub fn poke(&self) -> u32 {
        self.id
    }
}
