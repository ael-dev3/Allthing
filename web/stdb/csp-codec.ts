// SPDX-License-Identifier: AGPL-3.0-or-later
// SpacetimeDB 2.13's Product/Sum codec factories compile strings with
// Function(), which a strict CSP correctly rejects. Use cached closures for
// these factories; keep the SDK's primitive/array codecs and BSATN format.
// Import before generated bindings. No globals or CSP permissions change.
import { AlgebraicType, ProductType, SumType, Identity, ConnectionId, Timestamp, TimeDuration, Uuid } from "spacetimedb";

type Product = Parameters<typeof ProductType.makeSerializer>[0];
type Sum = Parameters<typeof SumType.makeSerializer>[0];
type Typespace = Parameters<typeof ProductType.makeSerializer>[1];
type Serialize = ReturnType<typeof ProductType.makeSerializer>;
type Deserialize = ReturnType<typeof ProductType.makeDeserializer>;
const serializers = new WeakMap<object, Serialize>();
const deserializers = new WeakMap<object, Deserialize>();

for (const factory of [ProductType, SumType]) {
  if (typeof factory.makeSerializer !== "function" || typeof factory.makeDeserializer !== "function") {
    throw new Error("SpacetimeDB codec API changed; review Allthing's CSP adapter before upgrading the SDK");
  }
}

ProductType.makeSerializer = (type: Product, space?: Typespace): Serialize => {
  const cached = serializers.get(type);
  if (cached) return cached;
  const fields: Array<[string, Serialize]> = [];
  const serialize: Serialize = (writer, value) => {
    for (const [name, write] of fields) write(writer, value[name]);
  };
  // Cache before traversing, so recursive types can refer to this closure.
  serializers.set(type, serialize);
  for (const field of type.elements) fields.push([String(field.name), AlgebraicType.makeSerializer(field.algebraicType, space)]);
  return serialize;
};

ProductType.makeDeserializer = (type: Product, space?: Typespace): Deserialize => {
  const cached = deserializers.get(type);
  if (cached) return cached;
  if (type.elements.length === 1) {
    const special: Record<string, Deserialize> = {
      __identity__: (reader) => new Identity(reader.readU256()),
      __connection_id__: (reader) => new ConnectionId(reader.readU128()),
      __timestamp_micros_since_unix_epoch__: (reader) => new Timestamp(reader.readI64()),
      __time_duration_micros__: (reader) => new TimeDuration(reader.readI64()),
      __uuid__: (reader) => new Uuid(reader.readU128()),
    };
    const name = String(type.elements[0].name);
    const read = Object.hasOwn(special, name) ? special[name] : undefined;
    if (read) { deserializers.set(type, read); return read; }
  }
  const fields: Array<[string, Deserialize]> = [];
  const deserialize: Deserialize = (reader) => {
    const value: Record<string, unknown> = {};
    for (const [name, read] of fields) Object.defineProperty(value, name, { value: read(reader), enumerable: true, writable: true, configurable: true });
    return value;
  };
  deserializers.set(type, deserialize);
  for (const field of type.elements) fields.push([String(field.name), AlgebraicType.makeDeserializer(field.algebraicType, space)]);
  return deserialize;
};

const isPair = (type: Sum, first: string, second: string) => type.variants.length === 2 && type.variants[0].name === first && type.variants[1].name === second;
SumType.makeSerializer = (type: Sum, space?: Typespace): Serialize => {
  const cached = serializers.get(type);
  if (cached) return cached;
  const variants: Array<{ name: string; write: Serialize }> = [];
  const serialize: Serialize = (writer, value) => {
    const tag = isPair(type, "some", "none") ? (value == null ? 1 : 0)
      : isPair(type, "ok", "err") ? ("ok" in value ? 0 : "err" in value ? 1 : -1)
      : variants.findIndex((variant) => variant.name === value.tag);
    if (tag < 0) throw new TypeError("Could not serialize sum type: unknown variant");
    writer.writeU8(tag);
    if (isPair(type, "some", "none")) { if (tag === 0) variants[0].write(writer, value); }
    else if (isPair(type, "ok", "err")) variants[tag].write(writer, tag === 0 ? value.ok : value.err);
    else variants[tag].write(writer, value.value);
  };
  serializers.set(type, serialize);
  for (const variant of type.variants) variants.push({ name: String(variant.name), write: AlgebraicType.makeSerializer(variant.algebraicType, space) });
  return serialize;
};

SumType.makeDeserializer = (type: Sum, space?: Typespace): Deserialize => {
  const cached = deserializers.get(type);
  if (cached) return cached;
  const variants: Array<{ name: string; read: Deserialize }> = [];
  const deserialize: Deserialize = (reader) => {
    const tag = reader.readU8();
    const variant = variants[tag];
    if (!variant) throw new TypeError("Could not deserialize sum type: unknown variant");
    if (isPair(type, "some", "none")) return tag === 0 ? variant.read(reader) : undefined;
    const value = variant.read(reader);
    if (isPair(type, "ok", "err")) return tag === 0 ? { ok: value } : { err: value };
    return { tag: variant.name, value };
  };
  deserializers.set(type, deserialize);
  for (const variant of type.variants) variants.push({ name: String(variant.name), read: AlgebraicType.makeDeserializer(variant.algebraicType, space) });
  return deserialize;
};
