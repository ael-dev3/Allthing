// SPDX-License-Identifier: AGPL-3.0-or-later
// Node-only protocol peer; the browser uses the unmocked, built SDK bridge.
import { BinaryReader, BinaryWriter, ConnectionId, Identity, Timestamp, t } from "spacetimedb";
import SourceReader from "../../node_modules/spacetimedb/src/lib/binary_reader";
import SourceWriter from "../../node_modules/spacetimedb/src/lib/binary_writer";
import { ClientMessage, ServerMessage } from "../../node_modules/spacetimedb/src/sdk/client_api/types";
import { NotificationState } from "../../web/stdb/module_bindings/types";
import VerifyNotificationFid from "../../web/stdb/module_bindings/verify_notification_fid_reducer";
import MarkNotificationsSeen from "../../web/stdb/module_bindings/mark_notifications_seen_reducer";

function frame(tag: string, value: unknown) {
  const writer = new SourceWriter(1024);
  ServerMessage.serialize(writer, { tag, value } as never);
  return Buffer.concat([Buffer.from([0]), Buffer.from(writer.getBuffer())]);
}
function rows(state?: { fid: number; seenAt: number }) {
  if (!state) return { sizeHint: { tag: "RowOffsets", value: [] }, rowsData: new Uint8Array() };
  const writer = new BinaryWriter(64);
  NotificationState.serialize(writer, { fid: BigInt(state.fid), seenAt: BigInt(state.seenAt) });
  const rowsData = writer.getBuffer();
  return { sizeHint: { tag: "FixedSize", value: rowsData.length }, rowsData };
}
export function initial(identity: string, token: string, connection: number) {
  return frame("InitialConnection", { identity: Identity.fromString(identity), connectionId: new ConnectionId(BigInt(connection)), token });
}
export function snapshot(requestId: number, querySetId: unknown, state?: { fid: number; seenAt: number }) {
  return frame("SubscribeApplied", { requestId, querySetId, rows: { tables: [{ table: "my_notification_state", rows: rows(state) }] } });
}
export function update(querySetId: unknown, before: { fid: number; seenAt: number } | undefined, after: { fid: number; seenAt: number }) {
  return frame("TransactionUpdate", { querySets: [{ querySetId, tables: [{ tableName: "my_notification_state", rows: [{ tag: "PersistentTable", value: { deletes: rows(before), inserts: rows(after) } }] }] }] });
}
export function client(bytes: Buffer | string) {
  return ClientMessage.deserialize(new SourceReader(typeof bytes === "string" ? Buffer.from(bytes) : bytes));
}
export function reducer(name: string, args: Uint8Array) {
  if (name === "verify_notification_fid") return t.row(VerifyNotificationFid).deserialize(new BinaryReader(args));
  if (name === "mark_notifications_seen") return t.row(MarkNotificationsSeen).deserialize(new BinaryReader(args));
  throw new Error(`Unexpected notification-sync reducer: ${name}`);
}
export function ok(requestId: number) {
  return frame("ReducerResult", { requestId, timestamp: new Timestamp(BigInt(Date.now()) * 1000n), result: { tag: "OkEmpty", value: {} } });
}
export function error(requestId: number, reason: string) {
  return frame("ReducerResult", { requestId, timestamp: new Timestamp(BigInt(Date.now()) * 1000n), result: { tag: "InternalError", value: reason } });
}
