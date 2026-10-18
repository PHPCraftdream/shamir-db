/**
 * DDL function & validator builders — stored functions, validators and
 * their bindings/folders. Split out of the former monolithic
 * `builders/ddl.ts`.
 */

import type {
  WriteOpKind,
  CreateFunctionOp,
  DropFunctionOp,
  RenameFunctionOp,
  CreateValidatorOp,
  DropValidatorOp,
  RenameValidatorOp,
  BindValidatorOp,
  UnbindValidatorOp,
  ListValidatorsOp,
  CreateFunctionFolderOp,
  RenameFunctionFolderOp,
  ListOp,
} from '../../types/schema/ddl.js';
import { repoOrDefault } from './ddl-schema.js';


/** Create (or replace) a stored function. */
export function createFunction(
  name: string,
  opts?: {
    source?: string;
    wasm?: string;
    replace?: boolean;
    /** `"public"` or `"private"` (absent → `"private"`). */
    visibility?: string;
    /** `"invoker"` or `"definer"` (absent → `"invoker"`). `"definer"` requires `hmac`. */
    security?: string;
    /** OS-seeded env-var secret grants. Non-empty requires `hmac`. */
    secret_grants?: string[];
    /** Hex HMAC tag, required IFF `security === "definer"` or non-empty `secret_grants`. */
    hmac?: string;
  },
): CreateFunctionOp {
  const op: CreateFunctionOp = {
    create_function: name,
    replace: opts?.replace ?? false,
  };
  if (opts?.source !== undefined) op.source = opts.source;
  if (opts?.wasm !== undefined) op.wasm = opts.wasm;
  if (opts?.visibility !== undefined) op.visibility = opts.visibility;
  if (opts?.security !== undefined) op.security = opts.security;
  if (opts?.secret_grants !== undefined && opts.secret_grants.length > 0) {
    op.secret_grants = opts.secret_grants;
  }
  if (opts?.hmac !== undefined) op.hmac = opts.hmac;
  return op;
}

/** Drop a stored function. */
export function dropFunction(
  name: string,
  opts?: { if_exists?: boolean },
): DropFunctionOp {
  const op: DropFunctionOp = { drop_function: name };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/** Rename a stored function. */
export function renameFunction(
  oldName: string,
  newName: string,
): RenameFunctionOp {
  return { rename_function: oldName, to: newName };
}

/** Create (or replace) a validator. */
export function createValidator(
  name: string,
  opts?: {
    source?: string;
    wasm?: string;
    replace?: boolean;
  },
): CreateValidatorOp {
  const op: CreateValidatorOp = {
    create_validator: name,
    replace: opts?.replace ?? false,
  };
  if (opts?.source !== undefined) op.source = opts.source;
  if (opts?.wasm !== undefined) op.wasm = opts.wasm;
  return op;
}

/** Drop a validator. */
export function dropValidator(
  name: string,
  opts?: { if_exists?: boolean },
): DropValidatorOp {
  const op: DropValidatorOp = { drop_validator: name };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/** Rename a validator. */
export function renameValidator(
  oldName: string,
  newName: string,
): RenameValidatorOp {
  return { rename_validator: oldName, to: newName };
}

/** Bind a validator to a table on specified write operations. */
export function bindValidator(
  name: string,
  table: string,
  ops: WriteOpKind[],
  priority: number,
  opts: {
    db: string;
    repo?: string;
  },
): BindValidatorOp {
  return {
    bind_validator: name,
    db: opts.db,
    repo: repoOrDefault(opts.repo),
    table,
    ops,
    priority,
  };
}

/** Unbind a validator from a table. */
export function unbindValidator(
  name: string,
  opts: {
    db: string;
    repo?: string;
    table: string;
  },
): UnbindValidatorOp {
  return {
    unbind_validator: name,
    db: opts.db,
    repo: repoOrDefault(opts.repo),
    table: opts.table,
  };
}

/** List validator bindings for a table. */
export function listValidators(
  table: string,
  opts: {
    db: string;
    repo?: string;
  },
): ListValidatorsOp {
  return {
    list_validators: table,
    db: opts.db,
    repo: repoOrDefault(opts.repo),
  };
}

/** Create a function folder by path segments. */
export function createFunctionFolder(
  segments: string[],
): CreateFunctionFolderOp {
  return { create_function_folder: segments };
}

/** Rename a function folder (and its descendant subtree) by path segments. */
export function renameFunctionFolder(
  from: string[],
  to: string[],
): RenameFunctionFolderOp {
  return { rename_function_folder: from, to };
}

export function listFunctions(opts?: { folder?: string }): ListOp {
  const op: ListOp = { list: 'functions' };
  if (opts?.folder !== undefined) {
    (op as { list: 'functions'; folder?: string }).folder = opts.folder;
  }
  return op;
}

export function listValidators_(): ListOp {
  return { list: 'validators' };
}

export function listFunctionFolders(opts?: { parent?: string }): ListOp {
  const op: ListOp = { list: 'function_folders' };
  if (opts?.parent !== undefined) {
    (op as { list: 'function_folders'; parent?: string }).parent =
      opts.parent;
  }
  return op;
}
