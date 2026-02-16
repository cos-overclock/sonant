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
| FR-02/06/09 | `ui::window`, `ui::state`, `domain::generation_contract` |
| FR-03 | `ui::window`, `ui::state`, `infra::midi::loader`, `plugin::live_midi_capture`, `app::midi_input_router` |
| FR-04 | `infra::llm::claude_client`, `app::generation_coordinator` |
| FR-05a〜g | `domain::generation_contract::GenerationMode`, `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state`, `infra::llm::prompt_builder::PromptBuilder` |
| FR-07 | `ui::window`, `ui::state`, `app::preview_state` |
| FR-10 | `ui::window`, `infra::secrets::api_key_store` |
| FR-11/12/15 | `app::history_service`, `app::variation_service`, `app::preset_service` |
| FR-13 | `infra::midi::exporter` |
| FR-14 | `plugin::transport_sync` |

### 4.3 FR-05a〜g モード別入力要件と実装モジュール（2026-02-16時点）

| FR | Mode | 参照MIDI要件（必須/任意） | 判定モジュール | プロンプト構築モジュール |
|---|---|---|---|---|
| FR-05a | `Melody` | 必須: なし / 任意: すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |
| FR-05b | `ChordProgression` | 必須: なし / 任意: すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |
| FR-05c | `DrumPattern` | 必須: なし / 任意: すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |
| FR-05d | `Bassline` | 必須: なし / 任意: すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |
| FR-05e | `CounterMelody` | 必須: `ReferenceSlot::Melody` を最低1件 / 任意: その他スロット | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |
| FR-05f | `Harmony` | 必須: `ReferenceSlot::Melody` を最低1件 / 任意: その他スロット | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |
| FR-05g | `Continuation` | 必須: いずれかの `ReferenceSlot` を最低1件 / 任意: 追加参照 | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` | `infra::llm::prompt_builder::PromptBuilder` |

### 4.4 PromptBuilder導入後の責務分担（FR-05関連）

| 責務 | 実装モジュール | 補足 |
|---|---|---|
| モード/参照要件の正規判定 | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements` | サーバー送信前の最終判定（真の受け入れ条件） |
| UI上の事前ガードと要件表示 | `ui::state::mode_reference_requirement`, `ui::state::mode_reference_requirement_satisfied`, `ui::window::SonantMainWindow::on_generate_clicked` | 生成実行前に不足要件を即時表示 |
| モード別テンプレート選択と参照MIDI埋め込み | `infra::llm::prompt_builder::PromptBuilder` | `GenerationMode` と `GenerationRequest.references` からLLM入力を構築 |
| プロバイダ呼び出し・リトライ | `app::generation_service::GenerationService`, `infra::llm::anthropic`, `infra::llm::openai_compatible` | PromptBuilder出力を使ってAPI実行 |
| 応答スキーマ検証と結果検証 | `infra::llm::schema_validator`, `domain::generation_contract::GenerationResult::validate` | JSON契約違反を検出し、UIへエラー返却 |

### 4.5 UI基準画面と責務分割（2026-02-16）

基準画面:

- メイン画面: `docs/image/sonant_main_plugin_interface/screen.png`
- 設定画面: `docs/image/sonant_api_&_model_settings/screen.png`

| 画面 | UI責務 | 状態・データ | 主実装モジュール |
|---|---|---|---|
| メイン画面（サイドバー） | Prompt/Mode/Model入力、入力トラック管理、生成候補選択、スライダー操作 | `prompt`, `mode`, `selected_model`, `input_tracks`, `selected_pattern`, `complexity`, `note_density` | `ui::window`, `ui::state`, `ui::request` |
| メイン画面（メインキャンバス） | Key/Scale/BPM編集、ピアノロール描画、再生位置表示、生成実行 | `generation_params`, `preview_candidates`, `playhead`, `ui_status` | `ui::window`, `app::preview_state`, `app::generation_job_manager` |
| 設定画面（Provider Configuration） | APIキー入出力、接続テスト、既定モデル設定 | `provider_settings`, `provider_status`, `default_model`, `context_window` | `ui::window`, `infra::secrets::api_key_store`, `infra::llm::provider_registry` |

UI一貫性ルール:

- ヘッダーのAPI状態表示（`API CONNECTED`）は設定画面のProvider状態と同一ソースを参照する。
- 設定の `Save & Close` 後は、`ui::state` を通じてメイン画面へ同期反映する。
- 画像基準からの逸脱を伴うUI変更は、`docs/image` と設計書（本書 + 詳細設計）を同時更新する。

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
3. MIDI Channelごとに `Recording` を有効化した状態でDAW再生を開始する
4. `live_midi_capture` がDAWからの入力イベントを受信し、`midi_input_router` がチャンネル割当に従って種別ごとの小節バッファへ振り分ける
5. `Recording` 有効チャンネルのみバッファを更新し、バッファ済み小節に再入力があった場合は当該小節を上書きし、入力がない小節の既存データは保持する
6. `GenerationCoordinator` が種別別バッファをファイル参照と同等の参照MIDIとして扱い、`MidiAnalyzer` と `PromptBuilder` に渡す
7. 生成処理は通常フロー（6.1）と同様に実行

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

## 11. 決定事項（2026-02-16）

- GUIはGPUI一本で進める
- LLM出力形式はJSON固定とする
- 生成履歴はJSONLで保存を開始する
- 参照MIDI解析のキー推定は外部ライブラリを採用する
- MIDI入力はファイル選択とリアルタイム入力の両方に対応する
- リアルタイム入力では入力種別ごとにMIDI Channelを設定可能とする
- RecordingモードはMIDI Channelごとに設定可能とする
- リアルタイム入力バッファは `Recording` 有効チャンネル + DAW再生中のみ更新し、小節単位で上書きする
- UI実装は `docs/image/sonant_main_plugin_interface/screen.png` と `docs/image/sonant_api_&_model_settings/screen.png` を基準とする

## 12. FR-05実装進捗チェックリスト（2026-02-16時点）

- [x] 7モード定義を `domain::generation_contract::GenerationMode` に集約
- [x] モード別参照要件を `GenerationRequest::validate_mode_reference_requirements` で検証
- [x] UIで7モードを選択できる（`ui::window` のモードセレクタ）
- [x] UIで参照要件不足を事前表示・ブロックできる（`ui::state`, `ui::window`）
- [x] `PromptBuilder` が7モードのテンプレートと参照MIDIイベント列をLLM入力へ反映
- [x] FR-05要件マトリクスを `domain` / `ui` / `infra::llm::prompt_builder` のユニットテストでカバー
- [x] 複数 `ReferenceSlot`のUIスロット選択・編集
- [ ] リアルタイム入力 + チャンネルマッピングUI（FR-03b/03c連携）
