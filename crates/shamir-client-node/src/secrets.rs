use zeroize::{Zeroize, Zeroizing};

pub(crate) struct Snapshots<T: Zeroize> {
    session: Option<Zeroizing<T>>,
    ticket: Option<Zeroizing<T>>,
}

impl<T: Zeroize> Snapshots<T> {
    pub(crate) fn new(session: T, ticket: Option<T>) -> Self {
        Self {
            session: Some(Zeroizing::new(session)),
            ticket: ticket.map(Zeroizing::new),
        }
    }

    pub(crate) fn session(&self) -> Option<&T> {
        self.session.as_deref()
    }

    pub(crate) fn ticket(&self) -> Option<&T> {
        self.ticket.as_deref()
    }

    pub(crate) fn clear(&mut self) {
        self.session = None;
        self.ticket = None;
    }
}

pub(crate) fn protect<T: Zeroize>(value: T) -> Zeroizing<T> {
    Zeroizing::new(value)
}

pub(crate) fn protect_before<T: Zeroize, R, E>(
    value: T,
    validate: impl FnOnce() -> Result<R, E>,
) -> Result<(Zeroizing<T>, R), E> {
    let protected = protect(value);
    let validated = validate()?;
    Ok((protected, validated))
}
