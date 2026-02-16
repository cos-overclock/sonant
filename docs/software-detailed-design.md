# Sonant ソフトウェア詳細設計

## 1. 目的

本書は `docs/software-architecture.md` を実装単位へ分解し、Rustコードへ直接落とし込める粒度で定義する。

## 2. 推奨コード構成

初期は単一crateでも開始できるが、将来の分離を見据え次のモジュール境界を固定する。

```text
src/
  plugin/
    clap_adapter.rs
    transport_sync.rs
    live_midi_capture.rs
  app/
    generation_coordinator.rs
    session_manager.rs
    preview_state.rs
    midi_output_router.rs
    midi_input_router.rs
    history_service.rs
    preset_service.rs
  domain/
    generation_mode.rs
    generation_request.rs
    generation_result.rs
    music_theory.rs
    errors.rs
  infra/
    llm/
      claude_client.rs
      prompt_builder.rs
    midi/
      loader.rs
      analyzer.rs
      assembler.rs
      exporter.rs
    storage/
      settings_store.rs
      history_store.rs
    secrets/
      api_key_store.rs
  ui/
    main_window.rs
    components/
      mode_selector.rs
      midi_slot_panel.rs
      channel_mapping_panel.rs
      parameter_panel.rs
      prompt_editor.rs
      piano_roll.rs
      candidate_navigator.rs
```

### 2.1 現行実装との対応（FR-05関連 / 2026-02-16時点）

| 観点 | 現行実装モジュール |
|---|---|
| モード定義 | `domain/generation_contract.rs` (`GenerationMode`) |
| モード別参照要件の最終判定 | `domain/generation_contract.rs` (`GenerationRequest::validate_mode_reference_requirements`) |
| UI事前判定/メッセージ | `ui/state.rs` (`mode_reference_requirement*`), `ui/window.rs` (`on_generate_clicked`) |
| モード別プロンプト構築 | `infra/llm/prompt_builder.rs` (`PromptBuilder`) |
| API呼び出し | `infra/llm/anthropic.rs`, `infra/llm/openai_compatible.rs` |

補足:

- 上記は実装済み（As-Is）の対応であり、セクション2のツリーは将来拡張を含む目標構成（To-Be）を示す。

## 3. ドメインモデル

### 3.1 基本型

```rust
pub enum GenerationMode {
    Melody,
    ChordProgression,
    DrumPattern,
    Bassline,
    CounterMelody,
    Harmony,
    Continuation,
}

pub struct GenerationParams {
    pub bpm: u16,
    pub key: MusicalKey,
    pub scale: ScaleType,
    pub density: u8, // 1..=5
    pub complexity: u8, // 1..=5
}

pub struct FileReferenceInput {
    pub path: String,
}

pub struct MidiReferenceEvent {
    pub track: u16,
    pub absolute_tick: u32,
    pub delta_tick: u32,
    pub event: String, // midlyイベントの文字列表現
}

pub struct MidiReferenceSummary {
    pub slot: ReferenceSlot,
    pub source: ReferenceSource,
    pub file: Option<FileReferenceInput>, // source=File時に必須
    pub bars: u16,
    pub note_count: u32,
    pub density_hint: f32,
    pub min_pitch: u8,
    pub max_pitch: u8,
    pub events: Vec<MidiReferenceEvent>, // source=File時は空禁止
}

pub struct GenerationRequest {
    pub request_id: String,
    pub model: ModelRef,
    pub mode: GenerationMode,
    pub prompt: String,
    pub params: GenerationParams,
    pub references: Vec<MidiReferenceSummary>,
    pub variation_count: u8, // Phase 3で利用
}

pub struct GenerationCandidate {
    pub id: String,
    pub events: Vec<MidiEvent>,
    pub bars: u16,
    pub score_hint: Option<f32>,
}
```

### 3.2 検証ルール

- `prompt` は空文字禁止（FR-02）
- `bpm` は `20..=300`
- `density/complexity` は `1..=5`
- モード別参照MIDI要件（FR-05a〜g）は以下の通り

| FR | Mode | 必須参照 | 任意参照 | 判定モジュール |
|---|---|---|---|---|
| FR-05a | `Melody` | なし | すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |
| FR-05b | `ChordProgression` | なし | すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |
| FR-05c | `DrumPattern` | なし | すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |
| FR-05d | `Bassline` | なし | すべての `ReferenceSlot` | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |
| FR-05e | `CounterMelody` | `ReferenceSlot::Melody` を最低1件 | その他スロット | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |
| FR-05f | `Harmony` | `ReferenceSlot::Melody` を最低1件 | その他スロット | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |
| FR-05g | `Continuation` | いずれかの `ReferenceSlot` を最低1件（継続元） | 追加参照 | `domain::generation_contract::GenerationRequest::validate_mode_reference_requirements`, `ui::state::mode_reference_requirement_satisfied` |

- `source=File` の参照MIDIは `events` が空であってはならない
- リアルタイム入力で使用する `channel` は `1..=16`
- `channel_mappings` は入力種別ごとに一意（重複チャンネル割当は不可）

## 4. 主要サービス設計

### 4.1 GenerationCoordinator

責務:

- `GenerationRequest` 受領
- 入力検証
- プロンプト構築
- LLM呼び出し
- MIDI候補への変換
- UI状態更新

I/F:

```rust
#[async_trait::async_trait]
pub trait GenerationCoordinator {
    async fn generate(
        &self,
        request: GenerationRequest,
        cancel: CancellationToken,
    ) -> Result<Vec<GenerationCandidate>, AppError>;
}
```

### 4.2 PromptBuilder

責務:

- モード別テンプレート選択
- 音楽理論パラメーターをプロンプトへ反映
- 参照MIDI特徴（音域/密度/リズム）を埋め込み
- 参照MIDIイベント列（`MidiReferenceSummary.events`）をLLM入力へ含める

責務分担（PromptBuilder導入後）:

| 担当 | 実装モジュール | FR-05との関係 |
|---|---|---|
| 入力検証の最終責任 | `domain/generation_contract.rs` | モード別必須参照の受け入れ判定を担保 |
| UIでの事前検証 | `ui/state.rs`, `ui/window.rs` | 不足参照を送信前に表示してUXを改善 |
| LLM入力文の構築 | `infra/llm/prompt_builder.rs` | モード別テンプレートと参照MIDI情報を組み立て |
| API送信とリトライ | `app/generation_service.rs`, `infra/llm/*` | PromptBuilder出力をプロバイダへ送信 |
| 応答検証 | `infra/llm/schema_validator.rs`, `domain/generation_contract.rs` | JSON契約違反・不正結果を検出 |

設計ポイント:

- LLM自由文出力を避けるため、JSONスキーマを明示して返却形式を固定
- JSON decode失敗時は1回まで自動リトライ

### 4.3 ClaudeClient

責務:

- APIリクエスト生成
- タイムアウト/リトライ/レート制限
- 応答検証

I/F:

```rust
#[async_trait::async_trait]
pub trait LlmGateway {
    async fn generate_midi_json(&self, prompt: String) -> Result<String, AppError>;
}
```

タイムアウト方針:

- 接続 + 応答全体で最大8秒（NFR-01達成のため）
- 失敗時は指数バックオフで最大2回再試行

### 4.4 MidiAnalyzer

責務:

- 参照MIDIから以下特徴を抽出
  - テンポ、拍子
  - ノート密度
  - 主音域
  - 拍頭アクセント傾向

用途:

- `CounterMelody/Harmony/Continuation` モード品質向上
- キー推定は外部ライブラリを利用して実装する

### 4.5 MidiOutputRouter

責務:

- 選択候補をDAW出力用イベントへ変換
- オーディオスレッド向けキューへ書き込み

制約:

- push-only（メモリアロケーションを最小化）
- 過負荷時は古い未適用データを破棄し最新を優先

### 4.6 MidiInputRouter / LiveMidiCapture

責務:

- DAWから受け取ったリアルタイムMIDIを入力種別へ振り分け
- 入力種別ごとの`channel_mappings`を管理
- 生成時に参照可能な短期バッファ（種別ごと）を保持

I/F:

```rust
pub trait MidiInputRouter {
    fn update_channel_mapping(&self, mappings: Vec<ChannelMapping>) -> Result<(), AppError>;
    fn push_live_event(&self, channel: u8, event: MidiEvent);
    fn snapshot_reference(&self, kind: LiveInputKind) -> Vec<MidiEvent>;
}
```

デフォルト割当:

- Melody: Channel 1
- Chord: Channel 2
- Drum: Channel 10
- Bass: Channel 3

## 5. 状態管理

```rust
pub enum UiState {
    Idle,
    LoadingReferences,
    Generating { started_at: std::time::Instant },
    PreviewReady { candidates: Vec<GenerationCandidate>, selected: usize },
    Applying,
    Error { message: String },
}
```

状態遷移:

- `Idle -> Generating -> PreviewReady`
- 失敗時は任意状態から `Error`
- `Error` は再生成で `Generating` に復帰可能

## 6. ユースケース別シーケンス

### 6.1 UP-1 プロンプトからMIDI生成

1. UIがモード/パラメーター/プロンプトを収集
2. `GenerationCoordinator::generate`
3. `PromptBuilder` でJSON出力指定付きプロンプト作成
4. `ClaudeClient` 呼び出し
5. `MidiAssembler` が `GenerationCandidate` へ変換
6. UIが `PreviewReady` 表示
7. `Apply` 実行で `MidiOutputRouter` へ転送

### 6.2 UP-2 参照MIDIを使った生成

1. `MidiLoader` がファイルを読み込み、サマリとイベント列を抽出
2. `MidiAnalyzer` が特徴抽出
3. 抽出結果とイベント列を `PromptBuilder` へ注入
4. 以後UP-1と同様

### 6.3 UP-3 続き生成

1. 既存MIDIの終端小節を解析
2. `ContinuationPolicy` が接続条件を設定
3. LLM出力後に「開始位置」「調性継続」を検証
4. 不整合なら再生成または自動補正

### 6.4 リアルタイム入力を使った生成

1. ユーザーが入力種別ごとにソースを「リアルタイム入力」に設定
2. `channel_mapping_panel` で種別ごとのMIDI Channelを設定
3. `live_midi_capture` がDAW入力を受信し `midi_input_router` に渡す
4. `midi_input_router` がチャンネルに応じて種別バッファへ振り分け
5. 生成時に種別バッファを `MidiReference::Live` として `GenerationRequest` に詰める
6. 以後は通常の生成シーケンス（6.1）で処理

## 7. 永続化設計

### 7.1 設定

- `settings_store` に以下を保存
  - 最終モード
  - 既定BPM/キー/スケール
  - 入力ソース設定（ファイル/リアルタイム）
  - リアルタイム入力のチャンネルマッピング
  - UI表示設定
- APIキー本体は保存しない（`api_key_store` 参照のみ保持）

### 7.2 APIキー

- OSシークレットストアに保存
- 取得失敗時はUIに再入力導線を表示

### 7.3 生成履歴（Phase 3）

保存項目:

- timestamp
- request hash
- mode
- prompt（ユーザー選択でマスク可）
- candidate summary（ノート数/長さ/キー）

初期実装案:

- JSONL + ローテーション（最大500件）

## 8. エラー設計

```rust
pub enum AppError {
    Validation(String),
    MidiParse(String),
    ApiAuth,
    ApiTimeout,
    ApiRateLimited,
    ApiResponseInvalid(String),
    Storage(String),
    Internal(String),
}
```

UI表示ポリシー:

- ユーザー修正可能エラー: 入力不足/ファイル形式不正/APIキー不正
- 一時障害エラー: タイムアウト/レート制限（再試行ボタン提示）
- 内部エラー: 簡潔な文言 + ログ参照ID

## 9. テスト設計

### 9.1 Unit Test

- `PromptBuilder` のモード別テンプレート生成
- `MidiAnalyzer` の特徴抽出
- `GenerationRequest` バリデーション
- `MidiInputRouter` のチャンネル振り分け

### 9.2 Integration Test

- 擬似LLMレスポンスから`PreviewReady`までのE2E
- 参照MIDIあり/なしの分岐
- `Apply`でMIDIイベント列が出力されること
- リアルタイム入力 + チャンネルマッピングで参照生成されること

### 9.3 非機能テスト

- オーディオスレッドでブロッキング呼び出しがないこと（静的確認 + ランタイム計測）
- 生成完了までの時間計測（P50/P95）
- メモリ100MB上限監視

## 10. 実装優先順

1. Phase 1:
- `GenerationMode::Melody` 固定で `Generate -> Preview -> Apply` を通す
- `ApiKeyStore` と `ClaudeClient` 最小実装

2. Phase 2:
- 7モード対応
- `MidiLoader/MidiAnalyzer` を統合
- DAW同期（テンポ/拍子）

3. Phase 3:
- 履歴・複数候補・プリセット・エクスポート

### 10.1 FR-05実装進捗チェックリスト（2026-02-16時点）

- [x] `GenerationMode` に7モードを定義
- [x] モード別必須参照の検証を `GenerationRequest::validate_mode_reference_requirements` に実装
- [x] `PromptBuilder` に7モードのテンプレート分岐を実装
- [x] `PromptBuilder` で `MidiReferenceSummary.events` をLLM入力へ注入
- [x] UIでモード別参照要件を表示し、未達時の生成をブロック
- [x] FR-05要件マトリクスを `domain` / `ui` / `infra::llm` のテストで検証
- [ ] UIで複数 `ReferenceSlot`（Melody以外）を個別に設定可能にする
- [ ] リアルタイム入力のモード別スロット設定とチャンネルマッピングUIを接続

## 11. 決定事項（2026-02-12）

- UIフレームワークはGPUI一本で進める
- LLM出力形式はJSON固定とする
- 履歴保存の初期方式はJSONLを採用する
- 参照MIDI解析のキー推定は外部ライブラリを採用する
- MIDI入力はファイル選択とリアルタイム入力の両方に対応する
- リアルタイム入力では入力種別ごとにMIDI Channelを設定可能とする
