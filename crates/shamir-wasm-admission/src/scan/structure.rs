use super::contract::{check_contract, ExportEntry};
use super::imports::scan_imports;
use super::operators::scan_operators;
use crate::types::ModuleHash;
use crate::types::{AdmissionError, AdmissionLimits, ModuleReport};
use wasmparser::{
    DataSectionReader, ElementItems, ElementSectionReader, Error, ExportSectionReader, FuncType,
    FunctionBody, FunctionSectionReader, GlobalSectionReader, ImportSectionReader,
    MemorySectionReader, RefType, TableSectionReader, TypeSectionReader,
};

fn wide(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn validation(e: Error) -> AdmissionError {
    AdmissionError::Validation(e.to_string())
}

fn parse(e: Error) -> AdmissionError {
    AdmissionError::Parse(e.to_string())
}

/// Counters and cap checks for one structural pass.
///
/// `limits` is `Some` only under the admission policy: every numeric cap is
/// skipped (and the contract data is not collected) when it is `None`, but the
/// counters are always filled. Section methods run after the validator has
/// accepted the section, so item-level reads are safe.
pub(crate) struct Structure<'l> {
    limits: Option<&'l AdmissionLimits>,
    type_count: u64,
    function_count: u64,
    import_count: u64,
    export_count: u64,
    global_count: u64,
    table_count: u64,
    element_segment_count: u64,
    element_item_count: u64,
    data_segment_count: u64,
    data_bytes_total: u64,
    memory_min_pages: u64,
    largest_function_body_bytes: usize,
    max_operators_in_function: u64,
    max_block_depth_seen: u64,
    // Contract data, collected only under admission.
    exports: Vec<ExportEntry>,
    func_type_indices: Vec<u32>,
    types: Vec<FuncType>,
}

impl<'l> Structure<'l> {
    pub(crate) fn new(limits: Option<&'l AdmissionLimits>) -> Self {
        Self {
            limits,
            type_count: 0,
            function_count: 0,
            import_count: 0,
            export_count: 0,
            global_count: 0,
            table_count: 0,
            element_segment_count: 0,
            element_item_count: 0,
            data_segment_count: 0,
            data_bytes_total: 0,
            memory_min_pages: 0,
            largest_function_body_bytes: 0,
            max_operators_in_function: 0,
            max_block_depth_seen: 0,
            exports: Vec::new(),
            func_type_indices: Vec::new(),
            types: Vec::new(),
        }
    }

    /// Fails with `err(limit, observed)` when a limit is active and exceeded.
    fn check(
        &self,
        pick: fn(&AdmissionLimits) -> usize,
        observed: u64,
        err: impl FnOnce(u64, u64) -> AdmissionError,
    ) -> Result<(), AdmissionError> {
        match self.limits {
            Some(limits) if observed > wide(pick(limits)) => Err(err(wide(pick(limits)), observed)),
            _ => Ok(()),
        }
    }

    pub(crate) fn on_type_section(
        &mut self,
        reader: TypeSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.type_count += u64::from(reader.count());
        self.check(
            |l| l.max_types,
            self.type_count,
            |limit, observed| AdmissionError::TooManyTypes { limit, observed },
        )?;
        if self.limits.is_none() {
            return Ok(());
        }
        for item in reader.into_iter_err_on_gc_types() {
            let ty = item.map_err(validation)?;
            let type_index = u32::try_from(self.types.len()).unwrap_or(u32::MAX);
            self.check(
                |l| l.max_params_and_results,
                wide(ty.params().len() + ty.results().len()),
                |limit, observed| AdmissionError::TooManyParamsAndResults {
                    type_index,
                    limit,
                    observed,
                },
            )?;
            self.types.push(ty);
        }
        Ok(())
    }

    pub(crate) fn on_import_section(
        &mut self,
        reader: ImportSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.import_count += u64::from(reader.count());
        self.check(
            |l| l.max_imports,
            self.import_count,
            |limit, observed| AdmissionError::TooManyImports { limit, observed },
        )?;
        let collect = self.limits.is_some();
        scan_imports(reader, collect.then_some(&mut self.func_type_indices))
    }

    pub(crate) fn on_function_section(
        &mut self,
        reader: FunctionSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.function_count += u64::from(reader.count());
        self.check(
            |l| l.max_functions,
            self.function_count,
            |limit, observed| AdmissionError::TooManyFunctions { limit, observed },
        )?;
        if self.limits.is_some() {
            for type_index in reader {
                self.func_type_indices.push(type_index.map_err(validation)?);
            }
        }
        Ok(())
    }

    /// At most one funcref table under every policy; size caps under admission.
    pub(crate) fn on_table_section(
        &mut self,
        reader: TableSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.table_count += u64::from(reader.count());
        if self.table_count > 1 {
            return Err(AdmissionError::TooManyTables {
                observed: self.table_count,
            });
        }
        for table in reader {
            let ty = table.map_err(validation)?.ty;
            if ty.element_type != RefType::FUNCREF {
                return Err(AdmissionError::NonFuncrefTable);
            }
            for size in [Some(ty.initial), ty.maximum].into_iter().flatten() {
                self.check(
                    |l| l.max_table_elements,
                    size,
                    |limit, observed| AdmissionError::TableTooLarge { limit, observed },
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn on_memory_section(
        &mut self,
        reader: MemorySectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        for memory in reader {
            let initial = memory.map_err(validation)?.initial;
            self.memory_min_pages = self.memory_min_pages.max(initial);
            self.check(
                |l| l.max_memory_min_pages,
                initial,
                |limit, observed| AdmissionError::MemoryTooLarge { limit, observed },
            )?;
        }
        Ok(())
    }

    pub(crate) fn on_global_section(
        &mut self,
        reader: &GlobalSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.global_count += u64::from(reader.count());
        self.check(
            |l| l.max_globals,
            self.global_count,
            |limit, observed| AdmissionError::TooManyGlobals { limit, observed },
        )
    }

    pub(crate) fn on_export_section(
        &mut self,
        reader: ExportSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.export_count += u64::from(reader.count());
        self.check(
            |l| l.max_exports,
            self.export_count,
            |limit, observed| AdmissionError::TooManyExports { limit, observed },
        )?;
        if self.limits.is_some() {
            for export in reader {
                let export = export.map_err(validation)?;
                self.exports
                    .push((export.name.to_string(), export.kind, export.index));
            }
        }
        Ok(())
    }

    pub(crate) fn on_element_section(
        &mut self,
        reader: ElementSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.element_segment_count += u64::from(reader.count());
        self.check(
            |l| l.max_element_segments,
            self.element_segment_count,
            |limit, observed| AdmissionError::TooManyElementSegments { limit, observed },
        )?;
        for element in reader {
            let items = match element.map_err(validation)?.items {
                ElementItems::Functions(items) => items.count(),
                ElementItems::Expressions(_, items) => items.count(),
            };
            self.element_item_count += u64::from(items);
            self.check(
                |l| l.max_element_items_total,
                self.element_item_count,
                |limit, observed| AdmissionError::TooManyElementItems { limit, observed },
            )?;
        }
        Ok(())
    }

    pub(crate) fn on_data_section(
        &mut self,
        reader: DataSectionReader<'_>,
    ) -> Result<(), AdmissionError> {
        self.data_segment_count += u64::from(reader.count());
        self.check(
            |l| l.max_data_segments,
            self.data_segment_count,
            |limit, observed| AdmissionError::TooManyDataSegments { limit, observed },
        )?;
        for data in reader {
            let data = data.map_err(validation)?;
            self.data_bytes_total += wide(data.data.len());
            self.check(
                |l| l.max_data_bytes_total,
                self.data_bytes_total,
                |limit, observed| AdmissionError::DataTooLarge { limit, observed },
            )?;
        }
        Ok(())
    }

    /// Caps and operator scan for one body; runs before the body is validated.
    pub(crate) fn on_function_body(
        &mut self,
        function_index: u32,
        body: &FunctionBody<'_>,
    ) -> Result<(), AdmissionError> {
        let size = body.range().len();
        self.largest_function_body_bytes = self.largest_function_body_bytes.max(size);
        self.check(
            |l| l.max_function_body_bytes,
            wide(size),
            |limit, observed| AdmissionError::FunctionBodyTooLarge {
                function_index,
                limit,
                observed,
            },
        )?;
        if self.limits.is_some() {
            let mut total: u64 = 0;
            for local in body.get_locals_reader().map_err(parse)? {
                let (count, _) = local.map_err(parse)?;
                total = total.saturating_add(u64::from(count));
                self.check(
                    |l| l.max_locals_per_function,
                    total,
                    |limit, observed| AdmissionError::TooManyLocals {
                        function_index,
                        limit,
                        observed,
                    },
                )?;
            }
        }
        let (operators, depth) = scan_operators(body, function_index, self.limits)?;
        self.max_operators_in_function = self.max_operators_in_function.max(operators);
        self.max_block_depth_seen = self.max_block_depth_seen.max(depth);
        Ok(())
    }

    /// Export contract check; a no-op unless the admission policy is active.
    pub(crate) fn check_contract(&self) -> Result<(), AdmissionError> {
        if self.limits.is_none() {
            return Ok(());
        }
        check_contract(&self.exports, &self.func_type_indices, &self.types)
    }

    pub(crate) fn into_report(
        self,
        byte_size: usize,
        hash: ModuleHash,
        abi_version: u32,
        origin_claim: Option<String>,
    ) -> ModuleReport {
        ModuleReport {
            byte_size,
            function_count: self.function_count,
            type_count: self.type_count,
            import_count: self.import_count,
            export_count: self.export_count,
            global_count: self.global_count,
            table_count: self.table_count,
            element_segment_count: self.element_segment_count,
            element_item_count: self.element_item_count,
            data_segment_count: self.data_segment_count,
            data_bytes_total: self.data_bytes_total,
            memory_min_pages: self.memory_min_pages,
            largest_function_body_bytes: self.largest_function_body_bytes,
            max_operators_in_function: self.max_operators_in_function,
            max_block_depth_seen: self.max_block_depth_seen,
            abi_version,
            origin_claim,
            hash,
        }
    }
}
