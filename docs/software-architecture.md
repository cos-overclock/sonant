# Sonant ソフトウェアアーキテクチャ設計

## 1. 目的と範囲

本書は `docs/product.md` を実装可能な形へ落とし込むための、Sonant の全体アーキテクチャを定義する。
対象は以下:

- CLAPプラグインとしてのホスト統合
- 自然言語 + 参照MIDI（ファイル/リアルタイム）を使う生成ワークフロー
- UI/非同期処理/セキュア保存を含むランタイム設計
- Phase 1〜4に対応する拡張性

## 2. 設計原則

1. オーディオスレッド非ブロッキング
- API通信、ファイルI/O、MIDI解析はすべてワーカー側で実行し、オーディオスレッドでは実施しない。

2. 生成処理の疎結合
- `GenerationCoordinator` を中心に、AI呼び出し、MIDI解析、MIDI組み立てを分離する。

3. プラットフォーム差分の吸収
- キー保存、ファイルパス、ウィンドウ管理はインフラ層で抽象化する。

4. 仕様トレーサビリティ
- FR/NFRをモジュール責務に直接マッピングし、フェーズ進行時の抜け漏れを防ぐ。

## 3. システムコンテキスト

```text
+--------------------- DAW Host (CLAP) ----------------------+
|  Track MIDI In --> [Sonant Plugin] --> Track MIDI Out      |
+-------------------------------------------------------------+
                               |
                               | HTTPS
                               v
                    +---------------------------+
                    | LLM API (Claude etc.)     |
                    +---------------------------+

Local resources:
- 参照MIDIファイル
- 生成履歴/プリセット
- APIキー(セキュアストレージ)
```

## 4. レイヤー構成

### 4.1 Layer定義

| レイヤー | 主責務 | 代表モジュール |
|---|---|---|
| Host Adapter Layer | CLAPライフサイクル、MIDI I/O、DAW同期 | `plugin::clap_adapter` |
| Application Layer | ユースケース実行、状態遷移、非同期ジョブ管理 | `app::generation_coordinator`, `app::session_manager` |
| Domain Layer | 生成リクエスト、候補、モード、検証ルール | `domain::*` |
| Infrastructure Layer | LLM API、MIDIパース、保存、鍵管理、ログ | `infra::llm`, `infra::midi`, `infra::storage`, `infra::secrets` |
| UI Layer | 画面描画、入力、プレビュー、操作イベント発火 | `ui::*` |

### 4.2 FRマッピング（主要）

| FR | 実現モジュール |
|---|---|
| FR-01/08 | `plugin::clap_adapter`, `app::midi_output_router` |
| FR-02/06/09 | `ui::main_window`, `domain::generation_request` |
| FR-03 | `ui::midi_slot`, `ui::channel_mapping_panel`, `infra::midi::loader`, `plugin::live_midi_capture`, `app::midi_input_router` |
| FR-04 | `infra::llm::claude_client`, `app::generation_coordinator` |
| FR-05a〜g | `domain::generation_mode`, `app::prompt_builder` |
| FR-07 | `ui::piano_roll`, `app::preview_state` |
| FR-10 | `infra::secrets::api_key_store` |
| FR-11/12/15 | `app::history_service`, `app::variation_service`, `app::preset_service` |
| FR-13 | `infra::midi::exporter` |
| FR-14 | `plugin::transport_sync` |

## 5. ランタイム構成

### 5.1 スレッド/実行コンテキスト

| コンテキスト | 役割 | 制約 |
|---|---|---|
| Audio Thread | DAWからの`process()`処理、MIDI入出力 | ブロッキング禁止、ロック最小化 |
| UI Thread | UI描画、ユーザー入力処理 | 高頻度再描画でも安定 |
| Worker Runtime | LLM API呼び出し、MIDI解析、履歴保存 | キャンセル可能、タイムアウト管理 |

### 5.2 通信方式

- UI -> App: コマンドキュー (`Generate`, `Apply`, `LoadMidi`, `StartLiveCapture`, `UpdateChannelMapping`, `SelectCandidate`)
- App -> UI: 状態ストア更新 (`Generating`, `PreviewReady`, `Error`)
- App -> Audio: lock-free ring bufferでMIDIイベントを受け渡し
- App -> Infra: 非同期関数呼び出し（`async/await`）

## 6. 主要データフロー

### 6.1 プロンプトから生成（UP-1）

1. UIが入力値から `GenerationRequest` を組み立て
2. `GenerationCoordinator` がバリデーション
3. `PromptBuilder` が構造化プロンプトを生成
4. `ClaudeClient` がAPI呼び出し
5. `MidiAssembler` がMIDIノート列を正規化
6. `PreviewState` が候補を保持し、UIのピアノロールを更新
7. `Apply` 実行時に `MidiOutputRouter` がDAWに出力

### 6.2 参照MIDIあり生成（UP-2/UP-3）

- `MidiLoader` でMIDIを読み込み、参照サマリ（小節数/ノート数/音域）とイベント列を抽出
- `MidiAnalyzer` がテンポ/キー推定、リズム特徴抽出
- `PromptBuilder` が参照特徴に加え、参照MIDIイベント列を `GenerationRequest.references` に含めてLLMに送信
- 続き生成時は `ContinuationPolicy` が既存末尾と接続整合性を検証

### 6.3 リアルタイムMIDI入力あり生成

1. UIで入力種別（例: メロディ、コード）ごとに入力ソースを「リアルタイム入力」に設定
2. 入力種別ごとにMIDI Channelを割当（例: メロディ=Channel 1、コード=Channel 2）
3. `live_midi_capture` がDAWからの入力イベントを受信し、`midi_input_router` がチャンネル割当に従って種別ごとのバッファへ振り分け
4. `GenerationCoordinator` が種別別バッファを参照MIDIとして扱い、`MidiAnalyzer` と `PromptBuilder` に渡す
5. 生成処理は通常フロー（6.1）と同様に実行

## 7. セキュリティ設計

- APIキーは平文ファイル保存しない（NFR-05）
- `ApiKeyStore` はOS標準シークレットストアを優先
  - macOS: Keychain
  - Windows: Credential Manager / DPAPI
- ログにプロンプト全文・APIキーを出力しない
- ネットワーク障害時はリトライ回数を制限し、UIに明示エラーを返す

## 8. パフォーマンス・品質目標

| 指標 | 目標 |
|---|---|
| 初回生成応答 | 10秒以内（NFR-01、通常ネットワーク条件） |
| プラグインメモリ使用量 | 100MB以下（NFR-03） |
| オーディオスレッドブロック | 0回（NFR-04） |

達成手段:

- リクエスト前処理の軽量化（トークン削減）
- 生成中プログレス表示 + キャンセル
- 参照MIDI解析結果のキャッシュ

## 9. クロスプラットフォーム方針

- Core/Application/Infrastructureはプラットフォーム非依存Rustで実装
- UIはGPUIを唯一のUIバックエンドとして採用する
- CLAPバイナリはWindows/macOS向けにCIで個別ビルド

## 10. フェーズ別アーキテクチャ到達点

| Phase | 到達点 |
|---|---|
| Phase 1 | Melodyモード、プロンプト入力、1候補生成、プレビュー |
| Phase 2 | 7モード、参照MIDI入力、MIDI出力 |
| Phase 3 | 履歴・複数候補・プリセット・エクスポート・DAW同期 |
| Phase 4 | リアルタイム生成、ローカルモデル、VST3/AU拡張 |

## 11. 決定事項（2026-02-12）

- GUIはGPUI一本で進める
- LLM出力形式はJSON固定とする
- 生成履歴はJSONLで保存を開始する
- 参照MIDI解析のキー推定は外部ライブラリを採用する
- MIDI入力はファイル選択とリアルタイム入力の両方に対応する
- リアルタイム入力では入力種別ごとにMIDI Channelを設定可能とする
