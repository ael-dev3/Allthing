// SPDX-License-Identifier: AGPL-3.0-or-later
// Node-only protocol peer: browser loads the unmocked production SDK bundle.
import { BinaryReader, BinaryWriter, Identity, ConnectionId, Timestamp, t } from "spacetimedb";
import SourceReader from "../../node_modules/spacetimedb/src/lib/binary_reader";
import SourceWriter from "../../node_modules/spacetimedb/src/lib/binary_writer";
import { ClientMessage, ServerMessage } from "../../node_modules/spacetimedb/src/sdk/client_api/types";
import { ChatMessage, Room, User } from "../../web/stdb/module_bindings/types";
import VerifyFid from "../../web/stdb/module_bindings/verify_fid_reducer";
import SendMessage from "../../web/stdb/module_bindings/send_message_reducer";
export const identity = Identity.fromString("c2".repeat(32));
const time = new Timestamp(1_800_000_000_000_000n);
export function frame(tag: string, value: unknown) {
  const writer = new SourceWriter(1024);
  ServerMessage.serialize(writer, { tag, value } as never);
  return Buffer.concat([Buffer.from([0]), Buffer.from(writer.getBuffer())]);
}
export function initial() {
  return frame("InitialConnection", { identity, connectionId: new ConnectionId(1n), token: "csp-test-token" });
}
function rows(table: string, schema: typeof User | typeof Room | typeof ChatMessage, value: unknown) {
  const writer = new BinaryWriter(1024);
  schema.serialize(writer, value as never);
  const rowsData = writer.getBuffer();
  return { table, rows: { sizeHint: { tag: "FixedSize", value: rowsData.length }, rowsData } };
}
export function snapshot(requestId: number, querySetId: unknown) {
  return frame("SubscribeApplied", { requestId, querySetId, rows: { tables: [
    rows("user", User, { identity, fid: 1234n, verified: false, online: true, lastSeen: time }),
    rows("room", Room, { id: 1n, name: "general", createdBy: identity, createdAt: time }),
    rows("chat_message", ChatMessage, { id: 1n, roomId: 1n, sender: identity, senderFid: 1234n, text: "A message decoded by the real SDK under CSP", sentAt: time }),
  ] } });
}
export function client(bytes: Buffer | string) {
  return ClientMessage.deserialize(new SourceReader(typeof bytes === "string" ? Buffer.from(bytes) : bytes));
}
export function reducer(name: string, args: Uint8Array) {
  return t.row(name === "verify_fid" ? VerifyFid : SendMessage).deserialize(new BinaryReader(args));
}
export function ok(requestId: number) { return frame("ReducerResult", { requestId, timestamp: time, result: { tag: "OkEmpty", value: {} } }); }
