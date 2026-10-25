// 服务端响应解码后的类型（BinaryProtocol.h 的 ParsedResponse / ServiceResponse 用）。
//
// 单独成头、刻意**不含任何 Win32 头**：KeyEventSink 的吃键判据要按响应类型分支，
// 判据收进纯函数头（如 CtrlAltCleanupPolicy.h）后才能用 g++ 在 Linux 上单测。
#pragma once

enum class ResponseType
{
    Ack,
    PassThrough,  // Key not handled, pass to system
    CommitText,
    UpdateComposition,
    ClearComposition,
    ClearCompositionThenPassThrough, // 收组合 + 把当前键交还宿主（重放）
    StatusUpdate,
    SyncHotkeys,
    Consumed,
    InsertTextWithCursor, // Insert text and position cursor
    MoveCursorRight,      // Move cursor right (smart skip)
    DeletePair,           // Delete left + right char (smart delete)
    ReplaceBackward,      // Delete N chars before caret + insert text (smart symbol)
    HoldComposition,      // Open composition with text + start auto-commit timer (smart symbol)
    CommitAndHold,        // Commit text then open composition with hold text + start timer
    CommitThenDefer,      // Commit text now, defer new composition (余码) to trigger-key keyup
    HostRenderSetup, // Host render setup (shared memory info)
    UiElementPage,   // 候选快照（CMD_UIELEMENT_QUERY 的响应）
    Error
};
