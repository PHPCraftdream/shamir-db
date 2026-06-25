/**
 * Wire type model — the single home for ShamirDB's platform-agnostic DTO
 * types. This barrel re-exports only; every type lives in a sibling file
 * (filter.ts, query.ts, connection.ts). Constructor/builder CODE lives
 * under `../builders/`, kept apart from these declarations.
 *
 * PLATFORM-AGNOSTIC.
 */

export type {
  FieldPath,
  FilterValue,
  FnCall,
  ExprOp,
  FilterExprValue,
  CondValue,
  Filter,
  ComputedFilter,
} from './filter.js';

export type {
  TableRefWire,
  AggFunc,
  AggregateField,
  SelectItem,
  Select,
  GroupBy,
  OrderDirection,
  NullsOrder,
  OrderByItem,
  OrderBy,
  Pagination,
  At,
  Temporal,
  ReadQuery,
} from './query.js';

export type { ConnectOptions, ResumeOptions } from './connection.js';

export type {
  WireValue,
  WriteValue,
  UpdateReturnMode,
  UpdateSelect,
  InsertOp,
  UpdateOp,
  SetOp,
  DeleteOp,
  WriteOp,
} from './write.js';

export type {
  HmacSigner,
  Retention,
  BufferConfigDto,
  BufferConfigPatch,
  PurgeScope,
  WriteOpKind,
  NumDto,
  ConstraintsDto,
  ForeignKeyDto,
  FkAction,
  FieldRuleDto,
  SetTableSchemaOp,
  AddSchemaRuleOp,
  RemoveSchemaRuleOp,
  GetTableSchemaOp,
  CreateDbOp,
  DropDbOp,
  CreateRepoOp,
  DropRepoOp,
  CreateTableOp,
  DropTableOp,
  CreateIndexOp,
  DropIndexOp,
  SetBufferConfigOp,
  GetBufferConfigOp,
  AlterBufferConfigOp,
  MigrationStatusOp,
  StartMigrationOp,
  CommitMigrationOp,
  RollbackMigrationOp,
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
  SetRetentionOp,
  PurgeHistoryOp,
  ChangesSinceOp,
  ListOp,
  DdlOp,
} from './ddl.js';

export type {
  ResourceRef,
  GroupRef,
  Resource,
  Action,
  Effect,
  Permission,
  ChmodOp,
  ChownOp,
  ChgrpOp,
  CreateGroupOp,
  DropGroupOp,
  RenameGroupOp,
  AddGroupMemberOp,
  RemoveGroupMemberOp,
  AccessTreeOp,
  CreateUserOp,
  DropUserOp,
  CreateRoleOp,
  DropRoleOp,
  GrantRoleOp,
  RevokeRoleOp,
  AdminOp,
} from './admin.js';

export type {
  CallOp,
} from './call.js';

export type {
  EventMask,
  SubscriptionSource,
  DeliverMode,
  SubscribeOp,
  UnsubscribeOp,
  PushKind,
  PushEnvelope,
} from './subscribe.js';

export type {
  SubBatchOp,
  BatchOpInput,
  QueryEntry,
  IsolationLevel,
  DurabilityLevel,
  BatchLimits,
  BatchRequest,
  QueryStats,
  PaginationInfo,
  QueryResult,
  TransactionInfo,
  BatchResponse,
} from './batch.js';

