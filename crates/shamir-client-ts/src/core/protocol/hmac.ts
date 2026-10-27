import { sha256 } from '@noble/hashes/sha256';
import type { Platform } from '../support/platform.js';
import type { ResourceRef, GroupRef, ChmodOp, ChownOp, ChgrpOp, CreateGroupOp, DropGroupOp, RenameGroupOp, AddGroupMemberOp, RemoveGroupMemberOp, CreateUserOp, DropUserOp, GrantRoleOp, RevokeRoleOp, SetSuperuserOp, SetReplicatorOp } from '../types/access/admin.js';
import type { DropDbOp, DropRepoOp, DropTableOp, DropIndexOp, StartMigrationOp, CommitMigrationOp, RollbackMigrationOp, SetRetentionOp, PurgeHistoryOp, CreateFunctionOp, CreateValidatorOp, DropFunctionOp, DropValidatorOp } from '../types/schema/ddl.js';

const utf8 = new TextEncoder();
const KEY_DOMAIN = utf8.encode('shamir-db hmac key v1\0');
const PREFIX = utf8.encode('shamir-db hmac canonical v2\0');
const TAGS = { drop_db:1, drop_repo:2, drop_table:3, drop_index:4, drop_user:5, start_migration:6, commit_migration:7, rollback_migration:8, grant_role:9, revoke_role:10, chmod:11, chown:12, chgrp:13, create_user:14, set_retention:15, purge_history:16, create_group:17, drop_group:18, rename_group:19, add_group_member:20, remove_group_member:21, create_function:22, set_superuser:23, set_replicator:24, create_scram_user:25, drop_function:26, drop_validator:27, create_validator:28, drop_replication_profile:29, drop_publication:30, drop_subscription:31 } as const;
function concatBytes(chunks:Uint8Array[]):Uint8Array { const out=new Uint8Array(chunks.reduce((n,c)=>n+c.length,0));let i=0;for(const c of chunks){out.set(c,i);i+=c.length;}return out; }
function hexEncode(bytes:Uint8Array):string { return Array.from(bytes,b=>b.toString(16).padStart(2,'0')).join(''); }
export function deriveSessionHmacKey(platform:Platform,sessionId:Uint8Array):Uint8Array{return platform.sha256(concatBytes([KEY_DOMAIN,sessionId]));}
export function computeTagHex(platform:Platform,key:Uint8Array,canonical:Uint8Array):string{return hexEncode(platform.hmacSha256(key,canonical));}
export function signCanonical(platform:Platform,sessionId:Uint8Array,canonical:Uint8Array):string{return computeTagHex(platform,deriveSessionHmacKey(platform,sessionId),canonical);}
/** Canonical DTO shape: unsigned (no required tag) and `cascade` as a plain flag. */
type CanonOf<T> = T extends unknown ? Omit<T, 'hmac' | 'cascade'> & { hmac?: string; cascade?: boolean } : never;
type BatchCanonicalOp = CanonOf<DropDbOp|DropRepoOp|DropTableOp|DropIndexOp|DropUserOp|StartMigrationOp|CommitMigrationOp|RollbackMigrationOp|GrantRoleOp|RevokeRoleOp|ChmodOp|ChownOp|ChgrpOp|CreateUserOp|SetRetentionOp|PurgeHistoryOp|CreateGroupOp|DropGroupOp|RenameGroupOp|AddGroupMemberOp|RemoveGroupMemberOp|CreateFunctionOp|DropFunctionOp|DropValidatorOp|CreateValidatorOp|{drop_replication_profile:string>;hmac?:string}|{drop_publication:string;hmac?:string}|{drop_subscription:string;hmac?:string};
export type DbCanonicalOp={op:'create_scram_user';name:string;password?:string;roles?:string[];hmac?:string}|(Omit<SetSuperuserOp,'hmac'>&{hmac?:string})|(Omit<SetReplicatorOp,'hmac'>&{hmac?:string});
class Encoder {
  private readonly chunks:Uint8Array[]=[PREFIX]; constructor(tag:number){this.raw(new Uint8Array([tag>>8,tag&255]));}
  private raw(bytes:Uint8Array){this.chunks.push(bytes);} u8(n:number){this.raw(new Uint8Array([n]));} u16(n:number){if(!Number.isInteger(n)||n<0||n>0xffff)throw new RangeError('u16 out of range');this.raw(new Uint8Array([n>>8,n&255]));}
  u32(n:number){this.raw(new Uint8Array([n>>>24,n>>>16,n>>>8,n]));} str(s:string){const b=utf8.encode(s);this.u32(b.length);this.raw(b);} bool(v:boolean){this.u8(v?1:0);}
  u64(v:number|bigint){if(typeof v==='number'&&!Number.isSafeInteger(v))throw new RangeError('unsafe u64 number; use bigint');let n=BigInt(v);if(n<0n||n>0xffffffffffffffffn)throw new RangeError('u64 out of range');const b=new Uint8Array(8);for(let i=7;i>=0;i--){b[i]=Number(n&255n);n>>=8n;}this.raw(b);}
  opt<T>(v:T|null|undefined,write:(x:T)=>void){if(v==null)this.u8(0);else{this.u8(1);write(v);}} list(v:string[]){this.u32(v.length);for(const x of v)this.str(x);} digest(v:string){this.raw(sha256(utf8.encode(v)));} finish(){return concatBytes(this.chunks);}
}
function writeResource(e:Encoder,r:ResourceRef){if('database'in r){e.u8(0);e.str(r.database);}else if('store'in r){e.u8(1);e.str(r.store[0]);e.str(r.store[1]);}else if('table'in r){e.u8(2);e.str(r.table[0]);e.str(r.table[1]);e.str(r.table[2]);}else if('function'in r){e.u8(3);e.str(r.function);}else if('function_folder'in r){e.u8(4);e.list(r.function_folder);}else e.u8(5);}
function writeGroup(e:Encoder,r:GroupRef){if('name'in r){e.u8(0);e.str(r.name);}else{e.u8(1);e.u64(r.id);}}
function writeScope(e:Encoder,s:import('../types/schema/ddl.js').PurgeScope){if('older_than'in s){e.u8(0);e.u64(s.older_than.timestamp);}else{e.u8(1);e.u64(s.older_than_age.age_secs);}}
function opTag(op:BatchCanonicalOp):keyof typeof TAGS {const names=Object.keys(TAGS) as Array<keyof typeof TAGS>;const found=names.find(k=>k in op);if(!found)throw new TypeError('unsupported canonical operation');return found;}
/** Whole-operation canonical v2 bytes; db supplies execution context for DB-scoped operation schemas. */
export function canonicalBatchOp(db:string,op:BatchCanonicalOp):Uint8Array {
 const tag=opTag(op),e=new Encoder(TAGS[tag]);
 switch(tag){
 case'drop_db':{const x=op as DropDbOp;e.str(x.drop_db);e.bool(Boolean(x.cascade));e.bool(Boolean(x.if_exists));break;}
 case'drop_repo':{const x=op as DropRepoOp;e.str(db);e.str(x.drop_repo);e.bool(Boolean(x.cascade));e.bool(Boolean(x.if_exists));break;}
 case'drop_table':{const x=op as DropTableOp;e.str(db);e.str(x.repo);e.str(x.drop_table);e.bool(Boolean(x.cascade));e.bool(Boolean(x.if_exists));break;}
 case'drop_index':{const x=op as DropIndexOp;e.str(db);e.str(x.repo);e.str(x.table);e.str(x.drop_index);e.bool(Boolean(x.if_exists));break;}
 case'drop_user':{const x=op as DropUserOp;e.str(x.drop_user);e.bool(Boolean(x.if_exists));break;}
 case'start_migration':{const x=op as StartMigrationOp;e.str(db);e.str(x.repo);e.str(x.start_migration);e.str(x.dst_repo);e.str(x.dst_engine);e.opt(x.dst_path,v=>e.str(v));break;}
 case'commit_migration':{const x=op as CommitMigrationOp;e.str(db);e.str(x.commit_migration);break;}
 case'rollback_migration':{const x=op as RollbackMigrationOp;e.str(db);e.str(x.rollback_migration);break;}
 case'grant_role':{const x=op as GrantRoleOp;e.str(x.grant_role);e.str(x.user);break;}
 case'revoke_role':{const x=op as RevokeRoleOp;e.str(x.revoke_role);e.str(x.user);break;}
 case'chmod':{const x=op as ChmodOp;writeResource(e,x.chmod);e.u16(x.mode);break;}
 case'chown':{const x=op as ChownOp;writeResource(e,x.chown);e.u64(x.owner);break;}
 case'chgrp':{const x=op as ChgrpOp;writeResource(e,x.chgrp);e.opt(x.group,v=>e.u64(v));break;}
 case'create_user':{const x=op as CreateUserOp;e.str(x.create_user);e.list(x.roles??[]);e.opt(x.database,v=>e.str(v));break;}
 case'set_retention':{const x=op as SetRetentionOp;e.str(db);e.str(x.repo);e.str(x.set_retention);for(const k of ['max_age_secs','max_count','min_count'] as const)e.opt(x.retention?.[k],v=>e.u64(v));break;}
 case'purge_history':{const x=op as PurgeHistoryOp;e.str(db);e.str(x.repo);e.str(x.purge_history);writeScope(e,x.scope);break;}
 case'create_group':{const x=op as CreateGroupOp;e.str(x.create_group);break;}
 case'drop_group':{const x=op as DropGroupOp;writeGroup(e,x.drop_group);e.bool(Boolean(x.if_exists));break;}
 case'rename_group':{const x=op as RenameGroupOp;writeGroup(e,x.rename_group);e.str(x.to);break;}
 case'add_group_member':{const x=op as AddGroupMemberOp;writeGroup(e,x.add_group_member);e.u64(x.user);break;}
 case'remove_group_member':{const x=op as RemoveGroupMemberOp;writeGroup(e,x.remove_group_member);e.u64(x.user);break;}
 case'create_function':{const x=op as CreateFunctionOp & {net_grants?:string[]};e.str(x.create_function);e.bool(x.replace);e.str(x.security??'invoker');e.opt(x.visibility,v=>e.str(v));e.list(x.secret_grants??[]);e.list(x.net_grants??[]);e.opt(x.source,v=>e.digest(v));e.opt(x.wasm,v=>e.digest(v));break;}
 case'drop_function':{const x=op as DropFunctionOp;e.str(x.drop_function);e.bool(Boolean(x.if_exists));break;}
 case'drop_validator':{const x=op as DropValidatorOp;e.str(x.drop_validator);e.bool(Boolean(x.if_exists));break;}
 case'create_validator':{const x=op as CreateValidatorOp;e.str(x.create_validator);e.bool(x.replace);e.opt(x.source,v=>e.digest(v));e.opt(x.wasm,v=>e.digest(v));break;}
 case'drop_replication_profile':{const x=op as {drop_replication_profile:string};e.str(x.drop_replication_profile);break;}
 case'drop_publication':{const x=op as {drop_publication:string};e.str(x.drop_publication);break;}
 case'drop_subscription':{const x=op as {drop_subscription:string};e.str(x.drop_subscription);break;}
 }
 return e.finish();
}
/** Canonical bytes for HMAC-bearing top-level DbRequest operations. */
export function canonicalDbRequest(op:DbCanonicalOp):Uint8Array {
 if(op.op==='create_scram_user'){const e=new Encoder(TAGS.create_scram_user);e.str(op.name);e.list(op.roles??[]);return e.finish();}
 if(op.op==='set_superuser'){const e=new Encoder(TAGS.set_superuser);e.str(op.user);e.bool(op.on);return e.finish();}
 const e=new Encoder(TAGS.set_replicator);e.str(op.user);e.bool(op.on);return e.finish();
}
