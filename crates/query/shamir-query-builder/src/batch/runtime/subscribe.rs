use shamir_query_types::batch::SubBatchOp;
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::Filter;
use shamir_query_types::subscribe::{DeliverMode, EventMask, SubscribeOp, SubscriptionSource};
use shamir_query_types::TableRef;

/// Builder for a single subscription source.
pub struct SourceBuilder {
    table: TableRef,
    filter: Option<Filter>,
    events: EventMask,
}

impl SourceBuilder {
    /// Start building a source for the given table.
    pub fn table(table: impl Into<TableRef>) -> Self {
        Self {
            table: table.into(),
            filter: None,
            events: EventMask::All,
        }
    }

    /// Add a filter condition — only events matching this filter are delivered.
    pub fn filter(mut self, f: Filter) -> Self {
        self.filter = Some(f);
        self
    }

    /// Restrict to specific event types.
    pub fn events(mut self, mask: EventMask) -> Self {
        self.events = mask;
        self
    }

    /// Build the source.
    pub fn build(self) -> SubscriptionSource {
        SubscriptionSource {
            table: self.table,
            filter: self.filter,
            events: self.events,
        }
    }
}

/// Builder for a [`SubscribeOp`].
pub struct Subscribe {
    sources: Vec<SubscriptionSource>,
    deliver: DeliverMode,
    initial: bool,
    from_version: Option<u64>,
}

impl Subscribe {
    /// Start with a single table source.
    pub fn table(table: impl Into<TableRef>) -> Self {
        Self {
            sources: vec![SourceBuilder::table(table).build()],
            deliver: DeliverMode::Records,
            initial: false,
            from_version: None,
        }
    }

    /// Start from an already-built source.
    pub fn source(source: SubscriptionSource) -> Self {
        Self {
            sources: vec![source],
            deliver: DeliverMode::Records,
            initial: false,
            from_version: None,
        }
    }

    /// Start from multiple sources.
    pub fn sources(sources: Vec<SubscriptionSource>) -> Self {
        Self {
            sources,
            deliver: DeliverMode::Records,
            initial: false,
            from_version: None,
        }
    }

    /// Add another source.
    pub fn add_source(mut self, source: SubscriptionSource) -> Self {
        self.sources.push(source);
        self
    }

    /// Deliver matching records (default).
    pub fn deliver_records(mut self) -> Self {
        self.deliver = DeliverMode::Records;
        self
    }

    /// Deliver only record keys.
    pub fn deliver_keys(mut self) -> Self {
        self.deliver = DeliverMode::Keys;
        self
    }

    /// Execute a reactive sub-batch on each event and deliver its result.
    pub fn deliver_batch(mut self, sub_batch: SubBatchOp) -> Self {
        self.deliver = DeliverMode::Batch(sub_batch);
        self
    }

    /// Call a stored function on each event and deliver its result.
    pub fn deliver_call(mut self, call: CallOp) -> Self {
        self.deliver = DeliverMode::Call(call);
        self
    }

    /// Request an initial snapshot of current records.
    pub fn with_initial(mut self) -> Self {
        self.initial = true;
        self
    }

    /// Resume from a specific changefeed version.
    pub fn from_version(mut self, v: u64) -> Self {
        self.from_version = Some(v);
        self
    }

    /// Build the [`SubscribeOp`].
    pub fn build(self) -> SubscribeOp {
        SubscribeOp {
            subscribe: self.sources,
            deliver: self.deliver,
            initial: self.initial,
            from_version: self.from_version,
        }
    }
}
