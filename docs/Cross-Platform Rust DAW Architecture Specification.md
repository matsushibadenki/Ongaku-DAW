# Cross-Platform Rust DAW Architecture Specification

## 1. プロジェクトの目的

macOS、Windows、Linux、iOS、Androidで可能な限り共通コードを使用する、本格的なDAW（Digital Audio Workstation）の基盤をRustで構築する。

UIにはReact + Vite + Tauri 2を使用する。

ただしTauriおよびWeb UIはDAWのリアルタイム処理には一切関与させない。

DAW本体は独立したRust Audio Engineとして設計し、TauriはRust Audio Engineを操作するためのApplication Shell / UI Adapterとして扱う。

最終的な基本構造は以下とする。

```text
React / Web UI
        │
        │ asynchronous IPC
        ▼
Tauri Adapter
        │
        ▼
Application / Control Layer
        │
        │ lock-free message passing
        ▼
Realtime Audio Engine
        │
        ├── DSP Graph
        ├── Transport
        ├── MIDI Scheduler
        ├── Automation
        ├── Mixer
        └── Plugin Processing
        │
        ▼
Platform Audio Backend
        │
 ┌──────┼────────┬────────┬─────────┐
 ▼      ▼        ▼        ▼         ▼
macOS  Windows  Linux     iOS     Android
```

設計思想としては「TauriでDAWを作る」のではなく、

**RustでDAWを作り、その上にTauri UIを載せる。**

これをプロジェクト全体の最上位原則とする。


# 2. 基本技術

フロントエンド:

```text
React
Vite
JavaScript
JSDocによる型情報
CSS
Canvas / WebGL / WebGPU
Tauri 2
```

バックエンド / Audio Engine:

```text
Rust
Cargo Workspace
CPAL
ringbuf等のSPSC lock-free queue
serde
tracing
```

CPALはクロスプラットフォームの低レベルAudio I/O層として使用する。

2026年時点でCPALは主に以下を扱える。

```text
macOS       CoreAudio
iOS         CoreAudio
Windows     WASAPI / ASIO
Linux       ALSA / JACK / PipeWire / PulseAudio
Android     AAudio
```

ただしAudio EngineをCPALそのものへ依存させてはならない。

CPALのさらに上に独自のAudioBackend traitを設ける。

将来的にCPALで不足する機能が発生した場合、

```text
CoreAudio direct backend
ASIO direct backend
WASAPI direct backend
AAudio direct backend
```

へ差し替えられる設計とする。


# 3. リアルタイム設計の最重要原則

macOS、Windows、Linux、iOS、Androidはいずれも一般的な意味でDAWアプリへHard Real-Time保証を提供するRTOSではない。

したがって本プロジェクトではHard Real-Timeではなく、

**極めて高い予測可能性を持つSoft Real-Time Audio Engine**

を構築する。

重要なのは平均処理速度ではない。

Audio callbackが指定されたdeadline内に処理を完了することを最優先する。

48 kHzの場合、

```text
256 samples = 約5.33 ms
128 samples = 約2.67 ms
64 samples  = 約1.33 ms
32 samples  = 約0.67 ms
```

となる。

Audio callbackは必ずこのdeadlineより十分短い時間で終了しなければならない。


# 4. スレッドモデル

最低限、以下の役割を完全分離する。

```text
Thread A
WebView / React UI

Thread B
Application Control

Thread C
Realtime Audio

Thread D...
Disk Streaming Workers

Thread E...
Plugin Scan / Background Processing

Thread F...
Waveform Analysis

Thread G...
Project Save / Autosave
```

最重要なのは、

```text
UI
↓
Control
↓
Realtime
```

という一方向の境界を明確にすることである。

ReactからAudio callbackを直接操作してはならない。


# 5. Realtime Audio Threadの禁止事項

Realtime Audio Threadでは原則として以下を禁止する。

```text
heap allocation
heap free
blocking mutex
RwLock待機
disk I/O
network I/O
database access
filesystem access
WebView access
Tauri IPC
JavaScript invocation
sleep
blocking channel
console output
通常のlogging
予測不能なOS API
```

可能な限り以下だけで構築する。

```text
preallocated memory
fixed-size buffer
atomic
lock-free SPSC queue
immutable data
bounded processing
SIMD
stack allocation
```

リアルタイムスレッド中で

```text
Vec::push()
String生成
HashMap拡張
Arcの最終drop
Boxの生成/破棄
```

などによって暗黙のallocation/freeが発生しないよう注意する。


# 6. Workspace構成

リポジトリはCargo Workspaceを中心として構成する。

```text
daw/
│
├── Cargo.toml
├── package.json
├── vite.config.js
│
├── crates/
│   │
│   ├── daw-core/
│   │   ├── transport/
│   │   ├── timeline/
│   │   ├── project/
│   │   └── command/
│   │
│   ├── audio-engine/
│   │   ├── engine/
│   │   ├── graph/
│   │   ├── buffer/
│   │   └── process/
│   │
│   ├── audio-backend/
│   │   ├── cpal/
│   │   └── platform/
│   │
│   ├── dsp/
│   │   ├── gain/
│   │   ├── pan/
│   │   ├── meter/
│   │   └── utilities/
│   │
│   ├── midi/
│   │   ├── event/
│   │   ├── scheduler/
│   │   └── device/
│   │
│   ├── mixer/
│   │
│   ├── automation/
│   │
│   ├── audio-file/
│   │
│   ├── streaming/
│   │
│   ├── plugin-host/
│   │
│   └── platform/
│
├── src-tauri/
│   ├── src/
│   │   ├── lib.rs
│   │   ├── commands/
│   │   ├── events/
│   │   └── state/
│   └── tauri.conf.json
│
├── src/
│   ├── app/
│   ├── components/
│   ├── timeline/
│   ├── mixer/
│   ├── piano-roll/
│   ├── transport/
│   ├── browser/
│   ├── meters/
│   ├── stores/
│   └── bridge/
│
└── tests/
    ├── realtime/
    ├── dsp/
    ├── transport/
    └── integration/
```

`audio-engine`からTauriをimportしてはならない。

依存方向は必ず

```text
Tauri
 ↓
Application
 ↓
Audio Engine
 ↓
DSP / MIDI / Platform abstraction
```

とする。

逆方向の依存は禁止する。


# 7. Audio Engine API

Audio EngineはTauriから完全に独立したRust library crateとする。

将来的に同じEngineを、

```text
Tauri DAW
CLI
Headless renderer
Audio Unit
VST
テスト環境
別Native UI
```

から利用できるようにする。

Audio Engineの基本インターフェイスは概念的に、

```text
Engine
├── prepare()
├── start()
├── stop()
├── process()
├── send_command()
├── get_snapshot()
└── shutdown()
```

とする。

`process()`がRealtime Contextである。


# 8. Audio Graph

トラック、Bus、FX、Instrument、MasterをAudio Graphとして管理する。

例:

```text
Audio Track 1
      │
      ▼
    EQ
      │
      ▼
 Compressor
      │
      ▼
   Track Bus ──────┐

Audio Track 2      │
      │            │
      ▼            │
    Reverb         │
      │            │
      └────────────┤
                   ▼
                Master
                   │
                   ▼
              Audio Output
```

グラフ構造そのものをRealtime Thread内で変更してはならない。

Control Threadで新しいGraphを構築し、

```text
Old Graph
   │
   │ atomic/safe swap
   ▼
New Graph
```

としてAudio block境界で切り替える。

Graphは可能な限りimmutable snapshotとして扱う。


# 9. Timelineと時間表現

DAW内部の基準時間としてOS clockやJavaScript timestampを使用しない。

Audio Engine内部では、

**sample position**

を絶対的な基準とする。

例:

```text
sample = 0
sample = 1
sample = 2
...
sample = 18432000
```

Transportは最低限、

```text
current_sample
sample_rate
tempo
time_signature
playing
recording
loop_range
```

を持つ。

Musical Timeは、

```text
Bar
Beat
Tick
```

として管理し、最終的にSample Positionへ変換する。

音の再生タイミングはfloat秒ではなく、可能な限り整数sample positionで管理する。


# 10. MIDI

MIDIイベントはJavaScript timerによって再生してはならない。

以下は禁止。

```text
setTimeout()
setInterval()
requestAnimationFrame()
```

これらはUI表示用途に限定する。

MIDI SchedulerはRust Audio Engine内部に置く。

MIDIイベントは概念的に、

```text
sample_position
event_type
channel
data
```

として保持する。

1 Audio block内にイベントが存在する場合、

```text
block start
     │
     ├── process 0..31
     │
     ├── MIDI event @ sample 32
     │
     ├── process 32..79
     │
     ├── MIDI event @ sample 80
     │
     └── process 80..127
```

のようにsample accurateまたはそれに準ずる処理ができる構造にする。


# 11. Parameter Automation

AutomationもUI更新頻度とAudio処理頻度を分離する。

UIでは、

```text
60 Hz
120 Hz
```

程度で表示してよい。

Audio Engineでは必要に応じてsample accurateなparameter eventへ変換する。

Automation dataそのものをAudio callbackで探索・編集してはならない。

Control/prepare処理でAudio block用イベントへ変換する。


# 12. UI → Engine通信

React → Tauri IPC → Control Threadまでは非リアルタイム通信でよい。

例:

```text
User moves volume fader

React
 ↓
Tauri IPC
 ↓
Control Thread
 ↓
Realtime Parameter Queue
 ↓
Audio callback
```

ただしマウス移動のたびに大量のTauri commandを同期的に発行する設計は避ける。

高頻度parameterについては、

```text
coalescing
throttling
shared state
bounded queue
```

を使用する。


# 13. Engine → UI通信

Realtime Threadから直接Reactへイベント送信してはならない。

例えばレベルメーターは、

```text
Audio Thread
 ↓
Atomic / Ring Buffer
 ↓
Meter Aggregator
 ↓
Control Thread
 ↓
UI
```

とする。

Audio Engineが48,000回/秒処理していても、UIへ送る必要はない。

Meter UIは30〜60 Hz程度へdownsampleする。


# 14. Web UIの描画方針

通常の設定画面、メニュー、ダイアログなどにはDOM/Reactを使用してよい。

しかし、

```text
Timeline
Waveform
Piano Roll
Automation Curve
Spectrum Analyzer
Large Mixer Meter Array
```

のような大量描画については、React DOM elementを数万個生成する設計は禁止する。

優先順位は、

```text
Canvas 2D
↓
WebGL
↓
WebGPU
```

を用途に応じて検討する。

ReactはCanvasの状態管理を担当し、描画ループそのものとは分離する。


# 15. Waveform

音声ファイル読み込み時にUIで直接PCM全体を解析しない。

Background Workerで、

```text
Peak
RMS
min/max
multi-resolution waveform
```

を生成する。

ズーム率ごとに適切なresolutionを使用する。

概念的には、

```text
Level 0  raw / high resolution
Level 1  2x
Level 2  4x
Level 3  8x
...
```

というWaveform Pyramidを保持する。

これにより数時間の音声でもタイムライン描画を軽量にする。


# 16. Audio File Streaming

巨大な音声ファイル全体をRAMへロードすることを前提としない。

以下の構造を使用する。

```text
Disk
 ↓
Streaming Worker
 ↓
Read Ahead Buffer
 ↓
Lock-free Ring Buffer
 ↓
Realtime Audio Thread
```

Realtime Threadはディスクを読まない。

再生位置付近をbackground threadが先読みする。


# 17. Recording

録音もAudio callbackから直接ファイルへ書き込まない。

```text
Audio Input
 ↓
Realtime callback
 ↓
Preallocated Ring Buffer
 ↓
Recording Writer Thread
 ↓
Audio File
```

とする。

Diskが一時的に遅くてもAudio callbackをblockさせない。


# 18. DSP

DSP内部ではinterleaved / planarなどのフォーマットをAudio Engine内部で統一する。

内部sample formatは初期段階では、

```text
f32
```

を基本とする。

必要になった段階でf64処理を追加できる設計にする。

DSP nodeは基本的に、

```text
prepare()
reset()
process()
```

というライフサイクルを持つ。

`prepare()`でメモリ確保を完了する。

`process()`ではallocationしない。


# 19. Mixer

最初のMixerでは、

```text
Gain
Pan
Mute
Solo
Meter
Send
Bus
Master
```

を実装する。

後に、

```text
PFL
AFL
Pre/Post Fader Send
VCA
Group
Control Room
Surround
```

を追加できる構造にする。


# 20. Plugin Host

Plugin HostはAudio Engineから独立crateにする。

Desktopでは将来的に、

```text
CLAP
VST3
Audio Unit (macOS)
```

を候補とする。

VST3はリアルタイムオーディオ処理を前提とした公式APIを持つ。

macOS/iOSではAppleのAudio Unit host APIを利用でき、AppleはAUv3 host向けAPIを提供している。

ただしPlugin HostはMVPには入れない。

まず内部DSP nodeだけでAudio Graphを完成させてからPlugin Hostを追加する。

理由は、外部plugin GUI、thread model、latency、crash isolation、state serializationなどがAudio Engine本体より複雑だからである。


# 21. Plugin Latency Compensation

将来的に各nodeは、

```text
latency_samples
```

を報告できるようにする。

Graph全体を解析して、

```text
Track A latency = 0
Track B latency = 512
Track C latency = 128
```

の場合、必要なdelayを挿入してMaster到達時刻を合わせる。

PDCは後付けしにくいため、初期Audio Graph APIからlatency propertyを用意する。


# 22. OS別Audio Backend

AudioBackend abstractionを定義し、

```text
AudioBackend
├── enumerate_devices()
├── open()
├── close()
├── start()
├── stop()
├── current_sample_rate()
├── buffer_size()
└── latency()
```

のような責務を持たせる。

実装候補:

```text
CPALBackend
CoreAudioBackend
WasapiBackend
AsioBackend
PipeWireBackend
AAudioBackend
```

最初はCPALBackendだけで実装する。

CPALは現在macOS/iOSのCoreAudio、WindowsのWASAPIおよびASIO、LinuxのALSA/JACK/PipeWire/PulseAudio、AndroidのAAudioを扱える。

低レイテンシ性能や特殊デバイス対応でCPALの抽象化が障害になった場合のみnative backendを追加する。


# 23. OS Thread Priority

Audio Threadは可能な範囲で各OSのリアルタイム/高優先度audio scheduling mechanismを利用できる構造にする。

ただしplatform-specific処理は、

```text
crates/platform/
```

へ隔離する。

Audio Engine本体へ、

```text
#[cfg(target_os = "...")]
```

を大量に散在させない。


# 24. Device変更

以下を必ず想定する。

```text
USB Audio Interface disconnect
Bluetooth disconnect
sample rate change
buffer size change
default output change
sleep / resume
mobile interruption
audio route change
```

Audio callback中に複雑な再構築は行わない。

Control Layerで、

```text
detect
↓
stop
↓
reconfigure
↓
prepare
↓
restart
```

するstate machineを実装する。


# 25. Engine State Machine

Audio Engineの状態は暗黙的に管理せず、明示的なstate machineとする。

```text
Uninitialized
     ↓
Prepared
     ↓
Stopped
     ↓
Running
     ↓
Stopping
     ↓
Stopped

Error
Reconfiguring
Suspended
```

不正な状態遷移はエラーとする。


# 26. Project Model

ProjectデータはReact stateを正本にしない。

Rust側のProject ModelをSingle Source of Truthとする。

UIはProjectのviewを保持する。

概念構造:

```text
Project
├── Metadata
├── Tempo Map
├── Tracks
│   ├── Regions
│   ├── Plugins
│   ├── Automation
│   └── Routing
├── Buses
├── Master
└── Assets
```

Project ModelとRealtime Engine Stateは同一オブジェクトにしない。

Project編集結果からRealtime Snapshotを生成する。


# 27. Undo / Redo

Undo / RedoはUI stateのコピーではなくCommand Systemとして実装する。

例:

```text
AddTrack
DeleteTrack
MoveRegion
ResizeRegion
ChangeGain
AddPlugin
SetTempo
```

これにより、

```text
execute
undo
redo
serialize
```

が可能になる。

Realtime ThreadにはCommand履歴を持ち込まない。


# 28. Autosave

Autosaveはbackground threadで行う。

Audio callbackからProject serializationを実行してはならない。

保存中にUIやAudioを停止しない設計を目標とする。


# 29. Crash Recovery

将来的に、

```text
project file
autosave snapshot
operation journal
```

を組み合わせる。

突然終了した場合でも直近状態を可能な限り復元できる構造にする。


# 30. Offline Rendering

Realtime再生とは別にOffline Render Modeを設ける。

```text
Realtime

Audio Device
     ↓
Engine
     ↓
Output
```

に対して、

```text
Offline

Engine
 ↓
process blocks as fast as possible
 ↓
Encoder
 ↓
File
```

とする。

Offline Renderingではwall clock deadlineを使用しない。

同じDSP Graphを可能な限り再利用する。


# 31. Mobile

Rust CoreはmacOS / Windows / Linux / iOS / Androidで可能な限り共有する。

しかしDesktopとMobileで完全な機能一致を要求しない。

特にPlugin Hostやbackground audio、filesystem、window managementはOS制約が異なる。

AppleはAUv3をhostするためのAPIを提供しているが、DesktopのVST3/CLAP hostをそのままiOSへ移植する設計にはしない。

したがって、

```text
Shared
Audio Engine
DSP
MIDI
Timeline
Project
Automation
Mixer

Platform dependent
Audio backend
Plugin host
Audio session
Window integration
Filesystem
Permissions
```

と明確に分ける。


# 32. 開発ターゲット順

実装順は、

```text
1. macOS
2. Windows
3. Linux
4. iOS
5. Android
```

とする。

これはコードをmacOS専用にするという意味ではない。

最初からplatform abstractionを使用するが、一度に5 OSをdebugしない。

最初にmacOS/CoreAudio系でAudio Engineの正しさを確立し、その後Windows/Linuxへ展開する。


# 33. Tauriの責務

Tauriが担当するもの:

```text
Window
Menu
Dialog
Frontend IPC
Application lifecycle
Filesystem permission integration
Native bridge
```

Tauriが担当してはいけないもの:

```text
Audio scheduling
MIDI scheduling
DSP
Audio Graph processing
Clock
Transport master timing
Audio recording pipeline
Plugin realtime processing
```

TauriはDAW Engineの一部ではない。


# 34. Reactの責務

Reactが担当するもの:

```text
UI state
Selection
Panels
Menus
Track controls
Visual editing
Keyboard shortcuts
Drag & Drop
Timeline viewport
Mixer controls
```

Reactが担当してはいけないもの:

```text
Master audio clock
Audio callback
MIDI clock
Sample scheduling
DSP
Realtime automation
Recording timing
```

React stateが遅延しても音が止まらない構造でなければならない。


# 35. Realtime Metrics

開発初期から以下を計測する。

```text
callback duration
callback deadline
CPU load
buffer underrun
buffer overrun
xrun
maximum callback time
average callback time
queue overflow
disk starvation
```

平均値だけでは不十分。

特に、

```text
maximum callback duration
99.9 percentile
99.99 percentile
```

を重要指標とする。

DAWで重要なのは平均速度ではなくworst-case latencyである。


# 36. Debug BuildとRealtime性能

Debug buildの性能を製品性能として評価しない。

Realtime testは必ずrelease buildでも行う。

ただしreleaseでのみ動作するコードにしてはならず、Debugではdeadline violationを検出・記録できるようにする。


# 37. Realtime Safety Test

専用test modeを用意する。

Realtime callback内で、

```text
unexpected allocation
unexpected lock
queue overflow
deadline miss
```

を可能な範囲で検出する。

CIではAudio Deviceを必要としないMock Backendを使用してAudio Engineを高速テストできるようにする。


# 38. Deterministic Test

Audio Engineへ固定入力を与え、

```text
Input
 ↓
DSP Graph
 ↓
Output
```

をofflineで処理し、期待したPCMと比較できるようにする。

Transport、MIDI、Automationについてもsample単位でテストする。


# 39. Benchmark

以下をbenchmark対象とする。

```text
32 sample buffer
64 sample buffer
128 sample buffer
256 sample buffer

44.1 kHz
48 kHz
96 kHz

1 track
16 tracks
64 tracks
128 tracks
256 tracks
```

最初から256 tracks対応を完成させる必要はないが、Architectureがtrack数に比例して不必要なallocationやIPCを発生させないことを確認する。


# 40. MVP

最初のMVPでは以下だけを完成させる。

```text
Audio device selection
Sample rate selection
Buffer size selection

Transport
Play
Stop
Seek
Loop

Audio Track
Audio Clip
Waveform

Mixer
Gain
Pan
Mute
Solo
Master

Basic MIDI data model

Basic automation architecture

Audio file streaming

Recording

Offline export

Realtime performance monitor
```

最初から以下を実装しない。

```text
VST3
Audio Unit
CLAP
Time Stretch
Pitch Correction
ARA
Video
Score Editor
Surround
Cloud Sync
AI
Collaboration
```

まずRealtime Audio Engineを完成させる。


# 41. MVP完成条件

MVPはUIが完成した時ではなく、Audio Engineが以下を満たした時に完成とする。

```text
64〜128 sample程度のbufferで安定動作する
長時間再生でAudio dropoutが発生しない
UIを激しく操作してもAudio callbackへ影響しない
Project保存中もAudioが停止しない
Waveform生成中もAudioが停止しない
複数trackをsample同期して再生できる
MIDI eventをAudio block内位置へ配置できる
録音中のdisk I/OがAudio threadをblockしない
Offline renderがRealtime Engineと同じ結果を出す
```

ハードウェアによって64 samplesが成立しない場合があるため、固定値そのものを製品保証とはしない。

重要なのはArchitecture上UIやdisk accessによる不定遅延をAudio callbackへ持ち込まないことである。


# 42. Codex実装ルール

Codexは以下の規則を必ず守ること。

1. Audio EngineからTauri/Reactへの依存を作らない。
2. Realtime `process()`内でheap allocationしない。
3. Realtime `process()`内でblocking lockを取得しない。
4. Audio callback内からdisk/network/UIへアクセスしない。
5. UI clockをAudio clockとして利用しない。
6. MIDI timingをJavaScript timerで処理しない。
7. Project ModelとRealtime Stateを分離する。
8. 大規模な変更を行う前にcrate境界を維持できるか確認する。
9. OS依存コードをplatform crateへ隔離する。
10. 各Realtime componentにはunit testまたはoffline testを追加する。
11. unsafe Rustは必要性を説明できる箇所に限定する。
12. 新しいdependencyを追加するときはRealtime pathへ与える影響を確認する。
13. panicがAudio callbackを跨いでアプリ全体へ伝播する設計を避ける。
14. bounded queueを基本とし、無制限queueをRealtime通信に使用しない。
15. realtime pathのコードレビューでは「高速か」より「処理時間が予測可能か」を優先する。


# 43. 最初にCodexへ実装させる内容

Phase 1ではUIを作り込まず、以下を作成する。

```text
Cargo Workspace
        ↓
AudioBackend abstraction
        ↓
CPAL backend
        ↓
Audio callback
        ↓
Audio Engine
        ↓
Gain DSP node
        ↓
Master Output
```

これでまず無音およびテストtoneを安定再生する。


Phase 2:

```text
Transport
Audio Graph
Track
Mixer
Audio Clip
File Streaming
```


Phase 3:

```text
React
Tauri Adapter
Timeline
Mixer UI
Transport UI
Waveform
```


Phase 4:

```text
MIDI Engine
MIDI Clip
Piano Roll
Automation
```


Phase 5:

```text
Recording
Offline Rendering
Project Save
Undo / Redo
Autosave
```


Phase 6:

```text
Plugin Host
PDC
External VST3 / CLAP / AU
```


# 44. 最初の技術的ゴール

最初に目指すものは「DAWらしいUI」ではない。

以下の実験を最初の成功条件とする。

```text
Rust process starts
        ↓
Audio device opens
        ↓
Realtime callback starts
        ↓
Audio Graph processes
        ↓
Test oscillator produces signal
        ↓
Gain node processes signal
        ↓
Master outputs signal
        ↓
React changes Gain
        ↓
Control Layer receives it
        ↓
Lock-free RT command changes Gain
        ↓
音切れなし
```

ここが成立してからDAW機能を追加する。


# 45. 将来的な高度化

Architectureは将来的に以下へ拡張できるようにしておく。

```text
Multicore Audio Graph
Parallel DSP
VST3
CLAP
Audio Unit
AUv3
Plugin Sandbox
Plugin Crash Isolation
PDC
Sample Accurate Automation
Tempo Map
MPE
MIDI 2.0
Elastic Audio
Time Stretch
Pitch Shift
ARA
GPU Visualization
WebGPU UI
Remote Control
Network Audio
AI Processing
```

ただし将来機能のためにMVPを複雑化してはならない。


# 46. 将来のMulticore Engine

初期版では1本のRealtime Audio Threadを基本とする。

Audio Graphが安定してから、

```text
          Track 1 ── Worker A ──┐
                                │
Input ─── Track 2 ── Worker B ──┼── Master
                                │
          Track 3 ── Worker C ──┘
```

のようなGraph dependencyに基づくparallel processingを検討する。

最初からthread-per-trackにはしない。

Thread数ではなくGraph dependencyとdeadlineを基準にschedulerを設計する。


# 47. RTOS的設計思想

このDAWはRTOSそのものを内蔵するわけではない。

しかしAudio Engine内部ではRTOSに近い思想を採用する。

すなわち、

```text
priority separation
bounded execution
deadline awareness
preallocation
non-blocking communication
predictable scheduling
minimal shared state
```

を徹底する。

通常OS上で可能な範囲において、

**小さなRealtime Systemがアプリケーション内部に存在する**

ようなArchitectureを目標とする。


# 48. 最終Architecture

完成形は以下とする。

```text
┌──────────────────────────────────────┐
│               React UI               │
│                                      │
│ Timeline / Mixer / Piano Roll        │
│ Browser / Inspector / Settings       │
└──────────────────┬───────────────────┘
                   │
              async IPC only
                   │
┌──────────────────▼───────────────────┐
│             Tauri Adapter            │
└──────────────────┬───────────────────┘
                   │
┌──────────────────▼───────────────────┐
│        Application Control Layer     │
│                                      │
│ Project / Commands / Undo / State    │
└──────────────────┬───────────────────┘
                   │
          lock-free / snapshot
                   │
┌──────────────────▼───────────────────┐
│        Realtime Audio Engine         │
│                                      │
│ Transport                            │
│ Audio Graph                          │
│ Mixer                                │
│ DSP                                  │
│ MIDI Scheduler                       │
│ Automation                           │
│ Plugin Processing                    │
└──────────────────┬───────────────────┘
                   │
┌──────────────────▼───────────────────┐
│          Audio Backend Layer         │
│                                      │
│ CPAL                                 │
│ Native backends when necessary      │
└────┬────────┬────────┬────────┬──────┘
     │        │        │        │
 CoreAudio  WASAPI   ASIO    PipeWire
     │        │                 │
 macOS/iOS Windows            Linux
     │
   AAudio
     │
 Android
```

この境界をプロジェクト全体を通して崩さないこと。


# 49. 最重要判断基準

機能追加時に迷った場合は、必ず次の問いを使う。

**「この処理が数十ミリ秒停止した場合、音声は止まるか？」**

YESなら、その処理をAudio Realtime Threadから排除する。

そしてRealtime設計については、

**平均的に速いことではなく、最悪時でもdeadlineを超えにくいこと**

を優先する。

これを本DAWのArchitectureにおける最優先原則とする。


# 50. Codexへの最初の指示

この仕様に従って、まずPhase 1だけを実装すること。

UIの完成を急がず、

1. Cargo Workspaceを作る。
2. Tauriから独立した`audio-engine` crateを作る。
3. `audio-backend` abstractionを作る。
4. CPAL backendを作る。
5. Realtime-safeなAudio callbackを作る。
6. Test oscillatorを作る。
7. Gain DSP nodeを作る。
8. Master OutputまでAudio Graphを通す。
9. RT callback duration / xrunを計測できるようにする。
10. Mock Audio Backendを使用したunit/integration testを作る。
11. その後Tauri 2 + ReactからGainだけを操作する最小UIを作る。
12. Reactを激しく操作しても音切れしないことを確認する。

Phase 1が正常に成立するまでPlugin Hostや高度なUIへ進まないこと。

最初から「機能の多いDAW」を作るのではなく、

**Realtime Audio Engineとして正しいDAW**

を構築すること。