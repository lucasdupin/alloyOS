// Run inside the native JavaScriptCore shell before browser integration.
// No Node.js, browser DOM, or alloyOS host bindings are required.
'use strict';
function assert(condition, label) {
    if (!condition) throw new Error('FAIL: ' + label);
}
const values = [1, 2, 3].map(value => value * 2);
assert(JSON.stringify(values) === '[2,4,6]', 'arrays and JSON');
const records = new Map([['answer', 42]]);
assert(records.get('answer') === 42, 'Map');
const bytes = new Uint8Array([1, 2, 255]);
assert(bytes.reduce((sum, value) => sum + value, 0) === 258, 'typed arrays');
let rejected = false;
try { JSON.parse('{'); } catch (error) { rejected = error instanceof SyntaxError; }
assert(rejected, 'exception identity');
Promise.resolve(21).then(value => {
    assert(value * 2 === 42, 'promise microtask');
    print('ALLOYOS_JSC_SMOKE_PASS');
});
