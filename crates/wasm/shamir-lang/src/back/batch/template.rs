//! Static DTO templates of the typed builder intrinsics.
//!
//! A batch plan IS the `BatchRequest` record of the wire format: a typed
//! record cell whose descriptor lists the fields in the order of the native
//! serialization (`id`, `transactional`, `queries`, `return_all`, `limits`,
//! `result_encoding`), so the shared encoder emits the exact DTO bytes. The
//! constant parts (defaults, the `select` template, operator strings, field
//! paths) are baked once into the static image.

use std::collections::BTreeMap;

use crate::back::ir::{cell_tag, IrBuilder, IrError};

/// `BatchLimits` defaults in serialization order (design section 4,
/// "Exact DTO mapping").
const LIMIT_FIELDS: [(&str, u64); 10] = [
    ("max_queries", 50),
    ("max_dependency_depth", 10),
    ("max_execution_time_secs", 30),
    ("max_result_size", 10_485_760),
    ("max_nesting_depth", 4),
    ("max_iterations", 1_000),
    ("max_total_queries", 10_000),
    ("max_total_work", 1_000_000),
    ("max_page_size", 100_000),
    ("max_offset", 1_000_000),
];

/// The `BatchRequest` field names in serialization order.
const REQUEST_FIELDS: [&str; 6] = [
    "id",
    "transactional",
    "queries",
    "return_all",
    "limits",
    "result_encoding",
];

/// Absolute addresses of the static builder templates plus the cache of
/// per-field path cells.
pub(crate) struct BatchStatics {
    /// The empty plan: a static `BatchRequest` record cell.
    pub batch_cell: u32,
    /// Descriptor of the `BatchRequest` record.
    pub request_descriptor: u32,
    /// The constant `select` template (`{ items: [{ type: "all" }], distinct: false }`).
    pub select_cell: u32,
    /// A static `true` cell (`return_result`).
    pub true_cell: u32,
    /// Static `"eq"` and `"and"` operator strings.
    pub eq_op: u32,
    pub and_op: u32,
    /// Entry and filter record descriptors.
    pub read_descriptor: u32,
    pub read_where_descriptor: u32,
    pub insert_descriptor: u32,
    pub eq_descriptor: u32,
    pub and_descriptor: u32,
    /// Static one-segment field path lists by field name.
    pub paths: BTreeMap<String, u32>,
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_string()).collect()
}

/// `(descriptor << 32) | region`: the payload of a record cell.
fn record_payload(descriptor: u32, region: u32) -> u64 {
    (u64::from(descriptor) << 32) | u64::from(region)
}

/// Appends raw bytes and returns `(length, absolute address)`.
fn text(builder: &mut IrBuilder, bytes: &[u8]) -> Result<(u32, u64), IrError> {
    let (length, address) = builder.static_text(bytes)?;
    Ok((length, address))
}

impl BatchStatics {
    /// Bakes every constant template into the static image.
    pub(crate) fn bake(builder: &mut IrBuilder) -> Result<Self, IrError> {
        // limits record (ten i64 cells)
        let limit_cells: Vec<(u32, u32, u64)> = LIMIT_FIELDS
            .iter()
            .map(|(_, value)| (cell_tag::I64, 0, *value))
            .collect();
        let limits_region = builder.static_region(&limit_cells)?;
        let limit_names: Vec<String> = LIMIT_FIELDS
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        let limits_descriptor = builder.record_descriptor(&limit_names)?;
        // the plan record
        let (encoding_len, encoding_addr) = text(builder, b"name")?;
        let request_region = builder.static_region(&[
            (cell_tag::NULL, 0, 0),
            (cell_tag::BOOL, 0, 0),
            (cell_tag::DYNAMIC_MAP, 0, 0),
            (cell_tag::BOOL, 0, 1),
            (
                cell_tag::RECORD,
                LIMIT_FIELDS.len() as u32,
                record_payload(limits_descriptor, limits_region),
            ),
            (cell_tag::STRING, encoding_len, encoding_addr),
        ])?;
        let request_descriptor = builder.record_descriptor(&names(&REQUEST_FIELDS))?;
        let batch_cell = builder.static_cell(
            cell_tag::RECORD,
            REQUEST_FIELDS.len() as u32,
            record_payload(request_descriptor, request_region),
        )?;
        // select template
        let (all_len, all_addr) = text(builder, b"all")?;
        let all_region = builder.static_region(&[(cell_tag::STRING, all_len, all_addr)])?;
        let all_descriptor = builder.record_descriptor(&names(&["type"]))?;
        let items_region = builder.static_region(&[(
            cell_tag::RECORD,
            1,
            record_payload(all_descriptor, all_region),
        )])?;
        let select_region = builder.static_region(&[
            (cell_tag::LIST, 1, u64::from(items_region)),
            (cell_tag::BOOL, 0, 0),
        ])?;
        let select_descriptor = builder.record_descriptor(&names(&["items", "distinct"]))?;
        let select_cell = builder.static_cell(
            cell_tag::RECORD,
            2,
            record_payload(select_descriptor, select_region),
        )?;
        let true_cell = builder.static_cell(cell_tag::BOOL, 0, 1)?;
        let (eq_len, eq_addr) = text(builder, b"eq")?;
        let eq_op = builder.static_cell(cell_tag::STRING, eq_len, eq_addr)?;
        let (and_len, and_addr) = text(builder, b"and")?;
        let and_op = builder.static_cell(cell_tag::STRING, and_len, and_addr)?;
        Ok(Self {
            batch_cell,
            request_descriptor,
            select_cell,
            true_cell,
            eq_op,
            and_op,
            read_descriptor: builder.record_descriptor(&names(&[
                "from",
                "select",
                "return_result",
            ]))?,
            read_where_descriptor: builder.record_descriptor(&names(&[
                "from",
                "select",
                "where",
                "return_result",
            ]))?,
            insert_descriptor: builder.record_descriptor(&names(&[
                "insert_into",
                "values",
                "return_result",
            ]))?,
            eq_descriptor: builder.record_descriptor(&names(&["op", "field", "value"]))?,
            and_descriptor: builder.record_descriptor(&names(&["op", "filters"]))?,
            paths: BTreeMap::new(),
        })
    }

    /// The static one-segment field path list cell of `name` (cached).
    pub(crate) fn path(&mut self, builder: &mut IrBuilder, name: &str) -> Result<u32, IrError> {
        if let Some(address) = self.paths.get(name) {
            return Ok(*address);
        }
        let (length, address) = text(builder, name.as_bytes())?;
        let region = builder.static_region(&[(cell_tag::STRING, length, address)])?;
        let cell = builder.static_cell(cell_tag::LIST, 1, u64::from(region))?;
        self.paths.insert(name.to_string(), cell);
        Ok(cell)
    }
}
