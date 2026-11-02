import AppKit
@main struct Tests {
 static func main() {
 let board=NSPasteboard.withUniqueName();defer {board.releaseGlobally()}
 let original=NSPasteboardItem();original.setString("original",forType:.string);original.setData(Data("{\\rtf1\\ansi original}".utf8),forType:.rtf);board.writeObjects([original]);
 let saved=clipboardSnapshot(board)!;assert(clipboardSnapshot(board,limit:1)==nil)
 board.clearContents();board.setString("dictation",forType:.string);let revision=board.changeCount
 restoreClipboard(board,revision:revision,items:saved)
 assert(board.string(forType:.string)=="original");assert(board.data(forType:.rtf)==Data("{\\rtf1\\ansi original}".utf8))
 board.clearContents();board.setString("dictation",forType:.string);let older=board.changeCount
 board.clearContents();board.setString("new user copy",forType:.string)
 restoreClipboard(board,revision:older,items:saved);assert(board.string(forType:.string)=="new user copy")
 let restoration = ClipboardRestoration(); var first = 0; var second = 0
 restoration.install(revision: 1) { first += 1 }; restoration.flush()
 restoration.install(revision: 2) { second += 1 }; restoration.finish(revision: 1)
 assert(first == 1 && second == 0); restoration.flush(); assert(second == 1)
 print("Rich clipboard restoration and newer-copy fencing passed")
 }
}
