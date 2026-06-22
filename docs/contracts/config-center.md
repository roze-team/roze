# Roze 配置中心（阶段1+2）实施收口说明

## 目标
- 支持 `Etcd -> Env -> File` 的配置优先级。
- Etcd 使用 v3 原生 `/v3/watch` 流式监听。
- 配置变更触发热更新并记录可观测事件。
- 解析失败不替换内存中的旧配置。

## 配置源定义

### Etcd
- 关键环境变量
  - `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS`: 逗号分隔 endpoint 列表
  - `ROZE_CONFIG_CENTER_KEY` 或 `ROZE_CONFIG_CENTER_ETCD_KEY`: etcd key
  - `ROZE_CONFIG_CENTER_NAMESPACE` + `ROZE_CONFIG_CENTER_APP`: 生成默认 key（可选）
- 优先级
  - 当存在 `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS` 时，优先使用 Etcd（第一位）
- 监听方式
  - 启动时通过 `/v3/kv/range` 读取初始值。
  - 运行期通过 `/v3/watch` 监听同一个 key 的 PUT 事件。
  - watch 记录 `mod_revision` 或 header `revision`；断线重连时使用 `start_revision = last_revision + 1` 恢复。
  - watch 流断开后自动重连；无法建立 watch 时回退到 `ROZE_CONFIG_CENTER_POLL_SECS` 间隔读取。

### 环境变量
- 关键环境变量
  - `ROZE_CONFIG_CENTER_ENV_KEY`: 直接读取该变量内容作为配置文本
- 优先级
  - 当无 Etcd 时使用该变量（第二位）

### 本地文件
- 关键环境变量
  - `ROZE_CONFIG_CENTER_FILE`: 指定回退配置文件（可选）
- 优先级
  - 当 Etcd 与 Env 均不可用时，当前服务文件作为回退源（第三位）
- 当服务配置文件存在且未设置任何中心变量时，默认仅做本地文件热加载（`file`）

## ReloadResult（可观测事件）
- `version`: 当前重载版本号
- `old_version`: 上一个版本号
- `hash`: 当前快照 hash
- `old_hash`: 上一个快照 hash
- `ts_millis`: 事件时间
- `source`: 来源（`etcd|env|file`）
- `namespace`: 配置命名空间（可选）
- `app`: 应用名（可选）
- `key`: 配置 key（可选）
- `changed`: 配置内容 hash 是否变化
- `diff`: 成功解析时的字段级差异数组，元素包含 `path`、`kind`、`old`、`new`
- `section_signatures`: 成功解析时的顶层 section 稳定签名数组，元素包含 `section` 和 `hash`
- `success`: 是否解析成功
- `error`: 失败原因（仅失败时）
- `config`: 成功时返回新配置，失败时为 `None`

## ConfigCenterChangeEvent（section 事件）

`ReloadResult::change_events()` 会把字段级 `diff` 聚合成 section 级事件，供应用按子系统做日志、审计或局部重建决策。

- `section`: 顶层配置段，例如 `gateway`、`kafka`、`registry`；根节点变更为 `root`，失败事件为 `*`
- `paths`: 当前 section 下发生变化的字段路径
- `diff`: 当前 section 下的字段级差异
- `section_hash`: 当前 section 的稳定签名；失败事件为空
- 其余字段继承自 `ReloadResult`：`version/old_version/hash/old_hash/source/namespace/app/key/changed/success/error`

## 字段边界

- 可选字段缺失时必须走默认值或 `None`，不能导致 reload 失败。
- `kafka.client_id` 缺失或显式为 `null` 时统一使用默认值 `roze-kafka`。
- section 签名按顶层配置段计算，字段顺序变化不应改变 hash；字段值变化才改变对应 section 的 hash。

## 触发约束
- debounce 默认 400ms（可通过 `ROZE_CONFIG_CENTER_DEBOUNCE_MS` 覆盖）
- Etcd 默认使用原生 watch；轮询默认 5s 仅用于 Env/File 或 Etcd watch 失败兜底（可通过 `ROZE_CONFIG_CENTER_POLL_SECS` 覆盖）
- 失败时不更新内存配置；仅记录失败事件
- 失败事件使用新快照 hash 和旧 hash，但 `diff` 为空，运行态继续保留上一份有效配置
- 成功事件会输出 `diff_paths`，用于审计“哪些字段发生变化”
- 应用可额外输出 `config_updated` section 事件，用于审计“哪些配置段发生变化”
- 默认配置格式为 `yaml`，可用 `ROZE_CONFIG_CENTER_FORMAT` 覆盖

## 在 `apps/user` 中的观察点
- `add_reload_listener` 日志：
  - 成功：`event=config.reload.applied`，包含 `version/old_version/hash/old_hash/diff_paths`
  - 失败：`event=config.reload.failed`，包含失败 hash、旧 hash 和错误原因
  - section 变更：`event=config_updated`，包含 `version/old_version/source/section/paths/changed/section_hash`
- Etcd 原生 watch 或 fallback poll 下发新配置后触发 Kafka pipeline 重建：
  - 条件：`kafka` 配置签名变更

## 快速示例

```bash
export ROZE_CONFIG_CENTER_ETCD_ENDPOINTS="127.0.0.1:2379"
export ROZE_CONFIG_CENTER_KEY="roze/user/config"
export ROZE_CONFIG_CENTER_POLL_SECS="2"
export ROZE_CONFIG_CENTER_DEBOUNCE_MS="400"

# Optional: for file fallback
export ROZE_CONFIG_CENTER_FILE="./apps/user/config.yaml"
```

## 验收脚本（最小）
- 仅本地文件启动成功
- 运行期注入 invalid etcd 配置：服务不退出并保持旧配置
- 恢复有效 etcd 配置后在窗口内更新签名
