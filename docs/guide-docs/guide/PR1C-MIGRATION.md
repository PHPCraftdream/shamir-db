# PR1c migration note: typed protocol options

The current client/server query protocol is v4. A server running this protocol rejects v3 requests; upgrade clients and servers together rather than relying on v3 fallback.

- Durability is a typed option with only `buffered` and `synced` values.
- Replace the former `async_index` durability option with `index_visibility: async`. The synchronous setting is `index_visibility: sync`. Do not use `async_index` as a current option.
- Strict option parsing rejects unknown keys in `BatchRequest` and `CreateRepoOp`, unknown top-level Config sections, and invalid `logging.level` values. Correct typos and remove unsupported options; they are errors, not ignored compatibility extensions.
- Supported transaction isolation modes are `snapshot`, `serializable`, and `pessimistic`. Pessimistic (level-3) isolation remains supported; only the string alias `level3` was dropped. Replace `level3` with `pessimistic`.

- `CreateIndexOp` strictly validates supported `vector_metric`, `vector_quantization`, and `fts_tokenizer` values; invalid or misspelled values are rejected.

The wire-level invalid-option response is `DbResponse::Error` with code `invalid_request` and a `message` field containing the detail. For query construction, use typed client builders; raw JSON examples in the protocol specification are illustrative wire-format references only.
