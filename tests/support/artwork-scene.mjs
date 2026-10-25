// The pose of the frame on show as the painter reads it: what a click on artwork paints in the app (`artworkScene` in
// src/app.mjs). The engine's groups of the frame (`editor.geometry`) carry no drawables for a 3D mesh or a movieclip's contents
// (docs/engine/evaluation.md, "The interface's frame"), so the tests that look at those read the pose built here.
import {buildGeometry} from '../../src/pivot/geometry.mjs';

export const artworkScene = editor => buildGeometry(editor.report, editor.evaluatedFrame);
