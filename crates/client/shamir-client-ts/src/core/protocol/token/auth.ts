import { tokenValueForTransport, type ApiToken } from '../../../entry/api-token.js';

export function authorizationHeader(token: ApiToken): string {
  return `Bearer ${tokenValueForTransport(token)}`;
}
