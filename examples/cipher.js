/**
 * Generates 256 unique substitution tables for the English alphabet.
 * Each table is a permutation of A-Z (26 letters).
 * Table[i][c] = (c * key) mod 26, where key = i-th prime starting from 500.
 */

function generateSubstitutionTables() {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
  const N = alphabet.length; // 26
  const tables = [];

  // We need 256 unique permutations.
  // A permutation: table[c] = (c * key) mod N is a bijection iff gcd(key, N) == 1.
  // For N=26, key must be coprime with 26 (not divisible by 2 or 13).

  // Collect 256 unique keys that are coprime with 26
  const keys = [];
  for (let k = 1; keys.length < 256; k++) {
    if (gcd(k, N) === 1) {
      keys.push(k);
    }
  }

  for (let i = 0; i < 256; i++) {
    const key = keys[i];
    const table = [];
    for (let c = 0; c < N; c++) {
      table.push(alphabet[(c * key) % N]);
    }
    tables.push(table);
  }

  return tables;
}

function gcd(a, b) {
  while (b) { [a, b] = [b, a % b]; }
  return a;
}

// --- Demo ---
const tables = generateSubstitutionTables();

console.log(`Generated ${tables.length} substitution tables, alphabet size = 26\n`);

// Show first 5 tables
for (let i = 0; i < 5; i++) {
  console.log(`Table ${i} (key=${i}): ${tables[i].join('')}`);
}
console.log('...');
console.log(`Table 255: ${tables[255].join('')}`);

// Verify all tables are unique permutations
const unique = new Set(tables.map(t => t.join('')));
console.log(`\nUnique tables: ${unique.size}`);

// Verify each table is a valid permutation (26 unique letters)
const allValid = tables.every(t => new Set(t).size === 26);
console.log(`All valid permutations: ${allValid}`);

// Encrypt example
function encrypt(plaintext, tableIndex, tables) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
  const table = tables[tableIndex];
  return plaintext.toUpperCase().split('').map(ch => {
    const idx = alphabet.indexOf(ch);
    return idx >= 0 ? table[idx] : ch;
  }).join('');
}

function decrypt(ciphertext, tableIndex, tables) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
  const table = tables[tableIndex];
  // Build reverse table
  const reverse = {};
  for (let i = 0; i < 26; i++) {
    reverse[table[i]] = alphabet[i];
  }
  return ciphertext.split('').map(ch => reverse[ch] || ch).join('');
}

const msg = 'HELLO WORLD';
const tableIdx = 42;
const encrypted = encrypt(msg, tableIdx, tables);
const decrypted = decrypt(encrypted, tableIdx, tables);
console.log(`\nPlaintext:  ${msg}`);
console.log(`Table:      ${tableIdx}`);
console.log(`Encrypted:  ${encrypted}`);
console.log(`Decrypted:  ${decrypted}`);
