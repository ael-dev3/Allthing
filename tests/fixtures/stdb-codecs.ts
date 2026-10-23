// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { AlgebraicType as A, BinaryWriter, BinaryReader, Identity, ConnectionId, Timestamp, TimeDuration, Uuid, t } from "spacetimedb";

export async function check() {
  const product = t.object("Bounds", { yes: t.bool(), i: t.i128(), u: t.u256(), name: t.string(), bytes: t.byteArray(), who: t.identity(), connection: t.connectionId(), at: t.timestamp(), duration: t.timeDuration(), uuid: t.uuid() });
  const sum = t.enum("Choice", { Empty: t.unit(), Data: product });
  const option = t.option(sum), result = t.result(sum, t.string());
  const value = { yes: true, i: -(1n << 127n), u: (1n << 256n) - 1n, name: "Unicode 🐻 / nul\0", bytes: new Uint8Array([0, 1, 255]), who: Identity.fromString("ab".repeat(32)), connection: new ConnectionId(123n), at: new Timestamp(-123n), duration: new TimeDuration(456n), uuid: new Uuid(123n) };
  const cases: Array<[any, any]> = [[product.algebraicType, value], [sum.algebraicType, { tag: "Data", value }], [sum.algebraicType, { tag: "Empty", value: {} }], [option.algebraicType, undefined], [option.algebraicType, { tag: "Data", value }], [result.algebraicType, { ok: { tag: "Data", value } }], [result.algebraicType, { err: "failure" }], [t.array(product).algebraicType, [value, value]], [t.row({ a: t.u8(), b: t.u64(), c: t.f64(), d: t.i32() }).algebraicType, { a: 255, b: (1n << 64n) - 1n, c: 2.5, d: -2147483648 }]];
  cases.push([A.Product({ elements: [{ name: "constructor", algebraicType: A.U32 }] }), { constructor: 42 }], [A.Product({ elements: [{ name: "toString", algebraicType: A.String }] }), { toString: "an ordinary field" }]);
  const encode = (type: any, value: any) => { const writer = new BinaryWriter(64); A.makeSerializer(type)(writer, value); return new Uint8Array(writer.getBuffer()); };
  const expected = cases.map(([type, value]) => { const bytes = encode(type, value); return { bytes, value: A.makeDeserializer(type)(new BinaryReader(bytes)) }; });
  const malformed: Array<[any, Uint8Array]> = [
    [product.algebraicType, new Uint8Array()],
    [A.String, new Uint8Array([255, 255, 255, 255])],
    [t.array(t.u64()).algebraicType, new Uint8Array([1, 0, 0, 0])],
  ];
  // Keep the SDK's bounds rejection for truncated products and attacker lengths.
  for (const [type, bytes] of malformed) assert.throws(() => A.makeDeserializer(type)(new BinaryReader(bytes)), RangeError);
  await import("../../web/stdb/csp-codec");
  for (const [type, bytes] of malformed) assert.throws(() => A.makeDeserializer(type)(new BinaryReader(bytes)), RangeError);
  for (const [index, [type, value]] of cases.entries()) {
    assert.deepEqual(encode(type, value), expected[index].bytes);
    assert.deepEqual(A.makeDeserializer(type)(new BinaryReader(expected[index].bytes)), expected[index].value);
  }
  // A recursive product / option shares the cached closure before resolving Ref.
  const linked: any = { tag: "Product", value: { elements: [] } };
  linked.value.elements = [{ name: "n", algebraicType: A.U32 }, { name: "next", algebraicType: A.Sum({ variants: [{ name: "some", algebraicType: A.Ref(0) }, { name: "none", algebraicType: A.Product({ elements: [] }) }] }) }];
  const space = { types: [linked] }, chain = { n: 1, next: { n: 2, next: undefined } };
  const writer = new BinaryWriter(64);
  A.makeSerializer(linked, space)(writer, chain);
  assert.deepEqual(A.makeDeserializer(linked, space)(new BinaryReader(writer.getBuffer())), chain);
  assert.throws(() => A.makeDeserializer(sum.algebraicType)(new BinaryReader(new Uint8Array([250]))), /unknown variant/);
  return cases.length;
}
