import test from "node:test";
import assert from "node:assert/strict";
import { Editor } from "../src/editor.mjs";
import { emptyDocument, createFrame, createObject } from "../src/demo.mjs";
import {
  canEditNativeProperty,
  getNativePropertyValue,
  beginNativePropertyEdit,
  previewNativePropertyEdit,
  commitNativePropertyEdit,
  cancelNativePropertyEdit,
} from "../src/native-property-edit.mjs";
const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-8, `${a} !== ${b}`);
const blank = () => {
  const e = new Editor({ demo: false });
  e.addFigure();
  e.selectedJoint = 0;
  e._undo = [];
  e._redo = [];
  e._historySize = 0;
  e.markSaved();
  return e;
};
function gesture(editor, key, value) {
  const token = beginNativePropertyEdit(editor, key);
  previewNativePropertyEdit(token, value);
  commitNativePropertyEdit(token);
  return token;
}

test("joint numeric edits capture only affected objects and form one pose undo", () => {
  for (const [key, value] of [
    ["joint_angle", 120.25],
    ["joint_length", 80.25],
    ["joint_bend", 40.125],
  ]) {
    const editor = blank(),
      before = structuredClone(editor.frame),
      other = editor.report.keyframes[1],
      count = editor._undo.length;
    const token = beginNativePropertyEdit(editor, key);
    for (let n = 0; n < 80; n++)
      previewNativePropertyEdit(token, value + n / 10);
    previewNativePropertyEdit(token, value);
    assert.equal(editor._undo.length, count);
    commitNativePropertyEdit(token);
    assert.equal(editor._undo.length, count + 1);
    assert.equal(editor._undo.at(-1).kind, "objects");
    near(getNativePropertyValue(editor, key), value);
    editor.undo();
    assert.deepEqual(editor.frame, before);
    assert.equal(editor.report.keyframes[1], other);
    editor.redo();
    near(getNativePropertyValue(editor, key), value);
  }
});

test("group translations and scaling are absolute from the starting snapshot", () => {
  for (const key of ["group_x", "group_y", "group_width", "group_height"]) {
    const editor = blank();
    editor.addFigure();
    editor.selectedUids = new Set(editor.frame.objects.map((o) => o.unique_id));
    editor._undo = [];
    editor._historySize = 0;
    const before = structuredClone(editor.frame),
      initial = getNativePropertyValue(editor, key),
      token = beginNativePropertyEdit(editor, key),
      value =
        initial + (key.endsWith("width") || key.endsWith("height") ? 80 : 120);
    for (let n = 0; n < 80; n++)
      previewNativePropertyEdit(token, value + n / 10);
    previewNativePropertyEdit(token, value);
    assert.equal(editor._undo.length, 0);
    commitNativePropertyEdit(token);
    const assertValue = () =>
      key === "group_width" || key === "group_height"
        ? near(editor.object.scale, value / initial)
        : near(getNativePropertyValue(editor, key), value);
    assertValue();
    assert.equal(editor._undo.length, 1);
    editor.undo();
    assert.deepEqual(editor.frame, before);
    editor.redo();
    assertValue();
  }
});

test("old Pivot FPS updates and undoes frame delay; property capability checks are contextual", () => {
  const editor = blank();
  editor.report.version = 4;
  editor.report.frame_delay_ms = 42;
  gesture(editor, "fps", 40);
  assert.equal(editor.report.frame_delay_ms, 25);
  editor.undo();
  assert.equal(editor.report.frame_delay_ms, 42);
  editor.selectedJoint = -1;
  assert.equal(canEditNativeProperty(editor, "joint_angle"), false);
  assert.equal(canEditNativeProperty(editor, "outline_width"), true);
  editor.select(null);
  assert.equal(canEditNativeProperty(editor, "outline_width"), false);
  assert.equal(canEditNativeProperty(editor, "duration"), true);
  assert.equal(canEditNativeProperty(editor, "root_x"), false);
});

test("unkeyed figure and camera values edit the native pose without creating scalar channels", () => {
  for (const [key, value] of [
    ["root_x", 240],
    ["root_y", 190],
    ["scale", 125.5],
    ["rotation", 112.25],
    ["figure_opacity", 50],
    ["camera_x", 70],
    ["camera_y", 45],
    ["camera_scale", 145.25],
    ["camera_angle", 35.125],
  ]) {
    const editor = blank(),
      before = structuredClone(editor.report),
      token = beginNativePropertyEdit(editor, key);
    for (let n = 0; n < 40; n++)
      previewNativePropertyEdit(
        token,
        value + (key.includes("opacity") ? n / 100 : n / 10),
      );
    previewNativePropertyEdit(token, value);
    assert.equal(editor._undo.length, 0);
    commitNativePropertyEdit(token);
    near(
      getNativePropertyValue(editor, key),
      key === "figure_opacity" ? (1 - 128 / 255) * 100 : value,
    );
    assert.equal(editor.report._styk?.animation, undefined);
    assert.equal(editor._undo.length, 1);
    const after = structuredClone(editor.report);
    editor.undo();
    assert.deepEqual(editor.report, before);
    editor.redo();
    assert.deepEqual(editor.report, after);
    const cancel = beginNativePropertyEdit(editor, key);
    previewNativePropertyEdit(cancel, value + 1);
    cancelNativePropertyEdit(cancel);
    assert.deepEqual(editor.report, after);
  }
});

test("numeric fallback edits a composed-only figure through one authored layer pose", () => {
  const report = emptyDocument(),
    figure = report.figure_types[0];
  report.keyframes = [
    createFrame([createObject(figure, 1, 100, 200)], 10),
    createFrame([createObject(figure, 2, 300, 200)], 10),
  ];
  const editor = new Editor({ demo: false }).load(report);
  editor.adoptNativePoseTrack({ kind: "layer", id: "layer-1" });
  const channel = editor.animationChannels[0];
  editor.setAnimationKey(channel.id, channel.keys[1].id, { tick: 5 });
  editor.seek(5);
  editor.select(2);
  const native = structuredClone(editor.report.keyframes),
    history = editor._undo.length;
  const token = beginNativePropertyEdit(editor, "root_x");
  assert.equal(token.initialValue, 300);
  for (let n = 0; n < 80; n++) previewNativePropertyEdit(token, 310 + n);
  assert.deepEqual(editor.report.keyframes, native);
  assert.equal(editor._undo.length, history);
  commitNativePropertyEdit(token);
  assert.equal(editor._undo.length, history + 1);
  assert.equal(editor.tick, 5);
  assert.equal(editor.object.root_x, 389);
  assert.equal(editor.animationChannel({ kind: "figure", uid: 2 }, "x"), null);
  assert.deepEqual(editor.report.keyframes, native);
  editor.undo();
  assert.equal(editor.object.root_x, 300);
  editor.redo();
  assert.equal(editor.object.root_x, 389);
  const cancel = beginNativePropertyEdit(editor, "root_y");
  previewNativePropertyEdit(cancel, 210);
  cancelNativePropertyEdit(cancel);
  assert.equal(editor.object.unique_id, 2);
  assert.equal(editor.object.root_y, 200);
  assert.equal(editor.tick, 5);
});

