// SPDX-License-Identifier: AGPL-3.0-or-later
// Independent protocol peer. The browser retains the real SDK and codecs.
import { BinaryReader, BinaryWriter, t } from "spacetimedb";
import SourceWriter from "../../node_modules/spacetimedb/src/lib/binary_writer";
import { ServerMessage } from "../../node_modules/spacetimedb/src/sdk/client_api/types";
import { AccountFilter, NotificationState } from "../../web/stdb/module_bindings/types";
import SetAccountFilter from "../../web/stdb/module_bindings/set_account_filter_reducer";
import VerifyNotificationFid from "../../web/stdb/module_bindings/verify_notification_fid_reducer";
import MarkNotificationsSeen from "../../web/stdb/module_bindings/mark_notifications_seen_reducer";

export { initial, client, ok, error } from "./notification-sync-wire";

function frame(tag: string, value: unknown) {
  const writer = new SourceWriter(1024);
  ServerMessage.serialize(writer, { tag, value } as never);
  return Buffer.concat([Buffer.from([0]), Buffer.from(writer.getBuffer())]);
}
function rows(table: string, values: unknown[]) {
  const schema = table === "my_account_filters" ? AccountFilter : NotificationState;
  const buffers = values.map(value => {
    const writer = new BinaryWriter(256);
    schema.serialize(writer, value as never);
    return Buffer.from(writer.getBuffer());
  });
  let size = 0;
  const offsets = buffers.map(buffer => { const offset = BigInt(size); size += buffer.length; return offset; });
  return { sizeHint: { tag: "RowOffsets", value: offsets }, rowsData: Buffer.concat(buffers) };
}
export function snapshot(requestId: number, querySetId: unknown, tables: { table: string; values: unknown[] }[]) {
  return frame("SubscribeApplied", { requestId, querySetId, rows: { tables: tables.map(({ table, values }) => ({ table, rows: rows(table, values) })) } });
}
export function update(querySetId: unknown, tables: { table: string; before: unknown[]; after: unknown[] }[]) {
  return frame("TransactionUpdate", { querySets: [{ querySetId, tables: tables.map(({ table, before, after }) => ({
    tableName: table, rows: [{ tag: "PersistentTable", value: { deletes: rows(table, before), inserts: rows(table, after) } }],
  })) }] });
}
export function unsubscribed(requestId: number, querySetId: unknown, tables: { table: string; values: unknown[] }[]) {
  return frame("UnsubscribeApplied", { requestId, querySetId, rows: { tables: tables.map(({ table, values }) => ({ table, rows: rows(table, values) })) } });
}
export function reducer(name: string, args: Uint8Array) {
  const schemas: Record<string, unknown> = {
    verify_notification_fid: VerifyNotificationFid,
    mark_notifications_seen: MarkNotificationsSeen,
    set_account_filter: SetAccountFilter,
  };
  if (!schemas[name]) throw new Error(`Unexpected moderation reducer ${name}`);
  return t.row(schemas[name] as never).deserialize(new BinaryReader(args));
}
