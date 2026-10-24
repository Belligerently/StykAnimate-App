/** The figure clipboard pasted into the frames `indices` by the editor's own range engine (`applyFigureRange`), for the tests of that
 * engine on native documents and on bare editors. The app's range paste is the engine session's command `pasteFiguresInFrames`. */
export function pasteInFrames(editor, indices, options = {}) {
  return editor.applyFigureRange({kind: 'paste', offset: options.offset, pasteMode: options.pasteMode ?? 'default', pasteDepth: options.pasteDepth}, {...options, indices});
}
