import { Batch } from '../../../shamir-client-ts/src/core/builders/write/batch.js';
import { Query } from '../../../shamir-client-ts/src/core/builders/read/query.js';
import type { ShamirClient } from '../../js/wrapper.js';

// The same builder-to-object FFI path documented for Node consumers.
export async function builderExample(nodeClient: ShamirClient): Promise<object> {
  const batch = Batch.create('rw')
    .add('rd', Query.from('items').asOfVersion(4294967296))
    .build();
  return nodeClient.execute('prod', batch);
}
