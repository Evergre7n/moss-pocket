"""量发布版的启动耗时和内存。

把 target/release/MOSS-Pocket.exe 拷到临时目录，用自测模式起若干次
（设环境变量 MOSS_POCKET_BENCHMARK：窗口不激活、不进任务栏、放到屏幕外），
读程序自己写出的数字，报告中位数和范围。全程只旁观，不发送任何键盘鼠标输入。

用法：python tools/benchmark.py <exe> <次数> [结果json]
只支持 Windows。
"""

import ctypes
import ctypes.wintypes as wt
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

CREATE_NO_WINDOW = 0x08000000
user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.GetForegroundWindow.restype = wt.HWND
user32.IsWindowVisible.argtypes = [wt.HWND]
user32.IsWindowVisible.restype = wt.BOOL
user32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
user32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
CALLBACK = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
user32.EnumWindows.argtypes = [CALLBACK, wt.LPARAM]

# 一行数字里各个字段从进程启动算起的时间点（毫秒）。
TIMELINE = [
    ("window_ready_ms", "窗口和显卡环境建好"),
    ("first_frame_ms", "第一帧上屏"),
    ("second_frame_ms", "第二帧"),
]


def windows_of(pid):
    """这个进程的顶层窗口：(句柄, 可见)。"""
    found = []

    @CALLBACK
    def visit(hwnd, _):
        owner = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid:
            found.append((hwnd, bool(user32.IsWindowVisible(hwnd))))
        return True

    user32.EnumWindows(visit, 0)
    return found


def measure(copy, run):
    """起一次程序，返回它自己写出的数字，加上旁观到的窗口情况。"""
    result = os.path.join(os.path.dirname(copy), f"bench-{run}.json")
    if os.path.exists(result):
        os.remove(result)
    start = time.perf_counter()
    proc = subprocess.Popen([copy], env=dict(os.environ, MOSS_POCKET_BENCHMARK=result), creationflags=CREATE_NO_WINDOW)
    (visible, offscreen, stolen) = (0, 0, 0)
    while proc.poll() is None and time.perf_counter() - start < 30:
        for hwnd, shown in windows_of(proc.pid):
            if shown:
                visible += 1
                rect = wt.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                offscreen += rect.right < 0 or rect.bottom < 0
                stolen += hwnd == user32.GetForegroundWindow()
        time.sleep(0.01)
    if proc.poll() is None:
        proc.kill()
    row = {"run": run, "exit": proc.returncode, "wall_ms": (time.perf_counter() - start) * 1000}
    if os.path.exists(result):
        with open(result, encoding="utf-8") as handle:
            row.update(json.load(handle))
    return row, visible, offscreen, stolen


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    (exe, runs) = (sys.argv[1], int(sys.argv[2]))
    base = os.path.join(tempfile.gettempdir(), "moss-pocket-bench")
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(base)
    copy = os.path.join(base, "MOSS-Pocket.exe")
    shutil.copyfile(exe, copy)

    rows, (visible, offscreen, stolen) = [], (0, 0, 0)
    for run in range(runs):
        row, shown, away, took = measure(copy, run)
        rows.append(row)
        visible, offscreen, stolen = visible + shown, offscreen + away, stolen + took
    shutil.rmtree(base, ignore_errors=True)

    missing = [row["run"] for row in rows if "first_frame_ms" not in row]
    print(f"起了 {runs} 次，没写出数字的：{missing or '无'}")
    for key, label in TIMELINE:
        values = [row[key] for row in rows if key in row]
        if values:
            print(f"{label}：中位 {statistics.median(values):.0f} ms（最快 {min(values):.0f}，最慢 {max(values):.0f}）")
    for key, label in [("ws_kb", "工作集"), ("private_kb", "私有内存"), ("fonts_kb", "其中字体占的私有内存")]:
        values = [row[key] / 1000 for row in rows if key in row]
        if values:
            print(f"{label}：中位 {statistics.median(values):.1f} MB")
    values = [row["fonts_ms"] for row in rows if "fonts_ms" in row]
    if values:
        print(f"装字体：中位 {statistics.median(values):.1f} ms")
    print(f"旁观：可见的窗口里在屏幕外的有 {offscreen}/{visible}，本程序窗口当上前台的次数 {stolen}")
    if len(sys.argv) > 3:
        with open(sys.argv[3], "w", encoding="utf-8") as handle:
            json.dump(rows, handle, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
