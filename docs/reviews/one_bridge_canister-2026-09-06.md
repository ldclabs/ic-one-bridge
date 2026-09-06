one_bridge_canister 完整代码审查与执行清单

修复状态：已按本次用户确认完成。当前实现、验证结果和升级注意事项见 [修复说明](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/docs/reviews/remediation-2026-09-06.md)。下文保留前次审查基准的触发条件与行号；F03 按明确的成本约束调整。

审查日期：2026-09-06。基准：`a2498a838869ef513dbc6bad3e6f90f53428a951`，crate 版本 0.5.2。

结论：仍存在资金处理、故障恢复和可用性问题。已有的双 RPC 核对、出金占位、轮次 generation、ICP 出金去重、稳定日志索引和轮询退避值得保留；它们没有覆盖下列边界。

本次逐行审阅了子库全部 16 个 Rust 文件，共 7,484 行，包括测试，以及 Cargo 配置和 185 行 Candid 接口；另外核对了相关依赖实现、构建流程和必要的历史格式。未使用 subagents，未向生产网络发送交易，也未修改生产 Rust 源码。

P1 表示应优先处理的资金安全、资源消耗或资金恢复问题；P2 表示可靠性、兼容性或规模问题。每项的触发条件与证据分别说明，不能把条件性风险理解为当前 PANDA 部署已发生损失。单元复现证明的是指定输入下的现有行为；真实 IC 消息调度、跨链重组及实际资金损失尚未在 PocketIC 或链上端到端复现。

验证结果：原有 61 个测试全部通过；`cargo fmt --all -- --check`、严格 Clippy（`-D warnings`）、锁定依赖的 release Wasm 构建均通过；从 Wasm 提取的 Candid 与仓库文件逐字节一致。另在临时副本执行了 13 个观察性复现测试，全部观察到所断言的现有行为。这 13 个测试不是“问题已修复”的回归测试。

资金与正确性执行清单：

- [x] **F01 · P1：为公开签名和 RPC 入口增加实际的资源预算。**

  位置：[api.rs:102](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api.rs:102)、[store.rs:725](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:725)、[store.rs:3089](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:3089)。

  触发：普通用户在派生地址放入足够的 token/native balance，循环调用 `erc20_transfer_tx`、`evm_transfer_tx`、`spl_transfer_tx` 或 `sol_transfer_tx`，拿到签名后不广播。余额一直不变，每次仍由 canister 支付 outcall 和门限签名费用。`ActiveBridgeUserGuard` 仅限制同时调用，返回后即可再调用。空钱包也可反复消耗余额检查之前的 RPC 开销。只有受授权的 `evm_sign` 入口主动接收签名成本所需 cycles。

  动作：在首次外部调用前落实每用户和全局预算；为独立签名服务收费或设置补贴额度；缓存、复用同一签名意图，避免相同请求反复签名；限制全局在途请求和 pending 数量。余额检查保留为交易可执行性检查。

  验收：同一有余额用户连续请求而不广播，达到额度后不再发出签名或 RPC 调用；多 principal 并发也不能绕过全局预算。门限签名按次收费见 [ICP cycle costs](https://docs.internetcomputer.org/references/cycle-costs/)。证据：完整入口与签名调用路径的静态审阅。

- [x] **F02 · P1：为 ICP 入金和管理提现补齐持久化转账意图及恢复流程。**

  位置：[store.rs:1757](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1757)、[store.rs:2616](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2616)、[api_admin.rs:266](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_admin.rs:266)、[helper.rs:106](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/helper.rs:106)。

  触发：`from_icp` 先扣款，返回后才创建 pending；其 `created_at_time`、`memo` 均为 None。若 ledger 已扣款，而回调解码失败或入队前 trap，canister 没有可恢复的入金记录，用户再次请求还会再次扣款。管理提现虽先预占 `total_withdrawn_fees`，但没有持久化提现记录及去重参数，所有 Err 都退回预占；结果不明确时可能重复提现，回调 trap 则可能留下无法定位的预占。

  动作：调用前记录 operation ID、ledger、完整参数、固定时间戳与 memo，支持按 ID 恢复；区分明确失败、明确成功、结果未知；结果未知时保留预占并查账或用相同参数重试。保留已有 ICP 出金的固定去重键，并让所有资金调用共用这一规则。进一步将 EVM/Solana 的待签名交易意图也记在签名调用前，以便处理签名结果未知。

  验收：在“扣款成功后回调 trap”“返回无法解码的成功响应”“升级发生在处理中”三个点注入故障，仍能定位该笔意图，并且最多扣款/付款一次。无界等待不能替代 journaling；ICP 官方说明了回调 trap 的提交边界和 [journaling 恢复流程](https://docs.internetcomputer.org/guides/security/inter-canister-calls/)。证据：源码与 CDK `CallErrorExt::is_clean_reject` 语义核对；尚未执行上述 PocketIC 故障测试。

- [x] **F03 · 已接受的设计约束：保留非复制请求，强化 provider 身份和双源确认。**

  用户明确选择成本优先的非复制模式，并使用官方 provider，对核心请求采用两家结果确认。实现保持 `is_replicated=false`；控制者批准独立 provider，同 host、path/port 别名和已识别同一供应商不能增加票数；核心金融证据需双源一致或保守结果。

  代码和文档不再宣称该模式能抵抗恶意单个 IC replica 篡改响应。它明确依赖服务请求的 IC 节点及受信任的官方 RPC。未切换复制请求，未把该选择作为待修复 P1 留下。

- [x] **F04 · P1：Solana 过期判定不能使用单方提供的未验证 deadline。**

  位置：[svm/rpc.rs:48](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/svm/rpc.rs:48)、[svm/rpc.rs:89](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/svm/rpc.rs:89)、[store.rs:2804](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2804)、[store.rs:2602](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2602)。

  触发：`getLatestBlockhash` 只相信第一家，将其 `lastValidBlockHeight` 原样保存；`expired` 随后让两家比较这个未经验证的数字。第一家可返回有效 blockhash 和过低 deadline，并暂扣已签名出金。只要检查时 blockhash 仍有效，两家真实状态均为 unknown 也会触发“已过期”，导致自动构建第二笔交易；若先后释放两笔不同 blockhash 的有效交易，可能重复付款。过高 deadline 则能使无效交易长期无法退出。

  动作：记录实际 blockhash、来源 slot/context 及验证过的有效期。过期证据必须绑定该 blockhash，并确认提供判定的链视图已经覆盖对应上下文；使用 `isBlockhashValid` 时不能把“节点尚未到达该 hash”误当过期。超出 provider 历史保留范围、无法证明旧交易未执行时，转入人工核对，不自动重建。

  验收：注入“有效 hash + 过低/过高 lastValidBlockHeight”时不能产生提前重建或无限 deadline；模拟暂扣后释放旧出金，最多付款一次。现有单元复现已证明：单方 deadline=1、两家高度=100、签名 unknown，会被判 expired；真实 hash 有效性作为场景前提，并未运行真实双付交易。

- [x] **F05 · P1：为 EVM gas/tip 增加多源验证和绝对成本上限。**

  位置：[evm.rs:122](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/evm.rs:122)、[store.rs:3072](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:3072)、[store.rs:2757](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2757)。

  触发：gas price 与 priority fee 都只取首个 RPC 答案，tip 还会上调 20%；没有单链或单笔经济上限。桥出金使用 `Funding::Trusted`。错误或恶意的高 tip 只要在钱包余额可承担范围内，会形成可上链的昂贵交易，直接消耗桥的 native gas 储备，并非只能造成交易失败。

  动作：设置每链 `max_priority_fee`、`max_fee` 和单笔/时间窗口的 gas 支出预算；异常报价停止签名并报告原因；结合多源报价与已确认区块费用数据。限制 gas_limit 不能替代 gas_price 上限。

  验收：注入极端但仍能由钱包支付的 tip，签名之前拒绝；正常费用波动仍能通过。复现确认：单 RPC 的极端报价被直接采用并进入费用计算；未花费真实 gas。

- [x] **F06 · P1：限定 Solana token 语义，按实际净入金记账。**

  位置：[api_admin.rs:124](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_admin.rs:124)、[svm/types.rs:99](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/svm/types.rs:99)、[store.rs:2373](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2373)。

  触发：注册 mint 仅检查 parsed type 和 decimals，未限制 token program/扩展；Solana 入金仅凭签名 finalized 就按完整 `icp_amount` 放款。带 TransferFeeConfig 的 Token-2022 mint 可被注册，`TransferChecked` 成功后桥收到的可支配余额却小于名义转账金额；出金也可能少到账。普通无转账费 SPL mint 不受这个特定条件影响。

  动作：短期只允许已明确支持的程序与扩展组合，拒绝尚未实现的 transfer-fee、transfer-hook 等语义；长期从最终交易的 token balance 变化/转账证据确认净入金，处理手续费上限和用户报价。Token-2022 ATA 租金按实际账户大小计算，不以 165 字节租金常量作为充分余额保证。

  验收：1% transfer-fee mint 入金 100，不能按收到 100 放款；不支持的扩展在启用链或扣款前拒绝；手续费配置改变后仍不能超额记账。复现已验证 fee 扩展不会被当前 mint 检查拒绝。[Solana 官方 transfer fees 文档](https://solana.com/docs/tokens/extensions/transfer-fees)确认 TransferChecked 会自动扣留该费用。

- [x] **F07 · P1：EVM 出金也要验证转账金额，不能仅看 status=1。**

  位置：[store.rs:2528](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2528)、[store.rs:2397](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2397)、[evm.rs:56](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/evm.rs:56)。

  触发：入金检查了 Transfer 事件，出金却在 `TxStatus::Confirmed(_)` 中丢弃 receipt，直接完成、归档并计入总额。若目标 token 的 transfer 返回 false 而不 revert，或者收取转账费，用户可能未收到约定金额，任务仍显示成功。标准 PANDA/OpenZeppelin 正常 transfer 不触发此路径，但本子库没有限制只能注册这种语义的 token。

  动作：出金复用统一到账验证器，核对 token、桥发送地址、目标地址和预计净额；不满足时保留用户债权并停止自动全额重付，交由核对处理。

  验收：status=1 但没有 Transfer、Transfer 指向错误地址、金额不足三种 receipt 均不能归档成功；标准成功交易正常归档。[ERC-20 标准](https://eips.ethereum.org/EIPS/eip-20)要求调用者处理 false 返回值。证据：完整出金与归档路径的静态验证。

- [x] **F08 · P1：统一终局证据，修复 EVM 重组与 Solana 过早失败判定。**

  位置：[evm.rs:35](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/evm.rs:35)、[evm.rs:242](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/evm.rs:242)、[store.rs:2875](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2875)、[svm/types.rs:75](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/svm/types.rs:75)。

  触发一：EvmReceipt 没有保存 blockHash，因此不同分叉的相同高度 receipt 可以比较为相等；先读 receipt、再 await 读 finalized height，也没有验证该 receipt 的区块仍在 canonical chain。触发二：`replaced` 用 latest nonce 就认定旧交易永远不能执行；该 nonce 对应的替代交易还可能被重组撤销。触发三：Solana 只要 `err` 非空便是 Failed，即使 confirmationStatus 还是 processed/confirmed；入金随后会被直接放弃和归档，重组后同笔交易仍有可能成功。

  动作：EVM 证据保留并比较 blockHash，再按最终链视图确认该区块；用符合配置 finality 的 nonce 判断替代，不能仅用 latest。Solana 必须 finalized 后才能形成终局 Failed，之前保留 Landed/Pending。把 finality 条件纳入同一个确认接口，防止成功、失败、死亡使用不同安全标准。

  验收：增加“receipt 后发生重组”“latest 替代交易被撤销”“processed error 后在 canonical fork 成功”三类场景。三个相关观察性单元测试均已复现当前错误的接纳/分类行为；真实重组影响需要链模拟验收。

可靠性、兼容性与规模执行清单：

- [x] **F09 · P2：消除三个在途 EVM 出金占满所有调度名额的饥饿问题。**

  位置：[store.rs:1047](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1047)。第一轮扫描先收集 EVM in-flight 任务，达到 ROUND_TASK_LIMIT=3 立即返回。如果 ETH、BNB、BASE 各一笔长期未确认，健康的 SOL/ICP 任务永远不会进入第二轮扫描；队列旋转也不能消除这个优先级饥饿。

  动作：按链公平调度、记录每任务下次可轮询时间，并独立保留每链 nonce 占用；给其他就绪任务保证服务名额。验收：三笔持续 pending 的 EVM 出金存在时，健康 SOL/ICP 任务在有限轮次内被执行。复现已经连续运行 100 轮，健康任务始终未被选中。

- [x] **F10 · P2：在轮次开始时主动设置超时恢复定时器。**

  位置：[store.rs:1584](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1584)、[store.rs:1919](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1919)、[store.rs:2262](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2262)。定时器启动时取走 FINALIZE_TIMER，下一次调度发生在整个 join_all 返回之后。一个无界 ledger 调用长期不返回、又没有新入金/管理员动作时，10 分钟 stale-lock 检查没有任何执行者；RoundGuard 只在 trap 清理时生效。

  动作：开始轮次、首次 await 前安排恢复定时器；每个完成任务尽快提交其结果，避免被同批慢任务拖住。保留 generation 和出金占位的防重复机制。验收：让 ledger 回调悬挂超过超时，并禁止新 ingress，定时器仍能恢复其他链处理；旧回调迟到不能覆盖新状态。证据：调度与 await 路径静态确认，待 PocketIC 验收。

- [x] **F11 · P2：JSON-RPC 的服务商错误应参与故障切换。**

  位置：[outcall.rs:224](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/outcall.rs:224)。HTTP 200 中的任何 error 都立即返回，rate limit、节点落后、服务端内部错误也不会尝试健康的后续 provider；“配置三家，一家故障仍工作”的目标因此不能保证。JSON-RPC error 分支和 `same` 的 Debug 错误文本也没有统一长度限制。

  动作：区分链执行拒绝和 provider 的限流/临时/能力错误；临时错误继续访问其他来源，所有错误文本统一截断并清理敏感内容。验收：首家返回 -32005 限流、后两家正常仍成功；限制错误存储大小。限流提前截断路径已单元复现。

- [x] **F12 · P2：首次扣款前验证目标金额可编码，重定向重试也检查精度。**

  位置：[store.rs:1633](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1633)、[store.rs:2125](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:2125)、[store.rs:3213](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:3213)。precision 检查只验证下采样余数；高 decimals 的乘法溢出和 Solana u64 上限直到出金构建才发现。ICP 入金已经扣除，随后形成永久无法构建的任务。`plan_retry_redirect` 也没有复用净额精度检查，改到低 decimals 链可能发生取整。

  动作：在 BridgePlan 中构造经过验证的源/目标链单位金额，检查非零、checked conversion、u64/u128 上限，重试改目标时复用同一验证。验收：20 个 token 从 8 decimals 转为 Solana 18 decimals 时，在扣款前拒绝超出 u64 的 2×10^19；不精确重定向不得产生截断付款。金额检查不足已单元复现。

- [x] **F13 · P2：管理配置在 await 后重新验证，并为未决任务固定资产身份。**

  位置：[api_admin.rs:55](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_admin.rs:55)、[api_admin.rs:124](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_admin.rs:124)、[api_init.rs:98](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_init.rs:98)。EVM/SVM 注册在外部请求前验证唯一性，回来后直接写入。两次正常管理调用可同时通过检查，覆盖配置或注册出两个相同 chain_id 的别名；按 chain_name 加的出金锁此时不能保护实际同一条链。升级还能修改 token_ledger，而未决任务没有记录原 ledger 身份。

  动作：提交配置时原子复检唯一性和配置版本，或使用注册占位；任务持久化 chain_id、token/ledger、config version。存在未决资金意图时禁止直接更换 ledger，除非执行明确迁移。验收：并发注册相同链 ID 仅一次成功；延迟回调不能覆盖新配置；升级不能使旧意图在另一个 ledger 上重试。证据：跨 await 与升级路径静态审阅。

- [x] **F14 · P2：公开 info 输出移除完整 RPC URL 中的凭据。**

  位置：[store.rs:253](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:253)、[api.rs:15](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api.rs:15)、[api_http.rs:73](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_http.rs:73)。StateInfo 原样公开所有 provider URL；付费 RPC 将 API key 放在路径/query 时，任何调用者都能直接读取。错误日志隐藏 host 之外部分不能解决此公开接口泄露。

  动作：公开 DTO 只输出 provider ID、host、健康状态；限制凭据暴露面，并避免给使用者承诺 canister 内明文 secret 对 replica 保密。验收：使用含 path/query token 的测试 URL，query、HTTP JSON/CBOR 和错误均不出现 token。[ICP HTTPS outcall 安全建议](https://docs.internetcomputer.org/guides/security/https-outcalls/)说明了节点可读取 canister 明文秘密。条件：确实使用带凭据 URL；未读取或验证生产凭据。

- [x] **F15 · P2：给 pending 与升级迁移设容量和每次执行预算。**

  位置：[store.rs:166](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:166)、[store.rs:1356](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1356)、[store.rs:1373](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1373)、[api_init.rs:81](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/api_init.rs:81)。pending 无容量限制并保存在 heap，每轮多次扫描且整队列重新分配；pre_upgrade 整体 CBOR 序列化状态，post_upgrade 的索引迁移/手续费恢复还可扫描全部历史。规模上升后会碰到内存或单次指令预算，阻止升级。

  动作：配置留在小型 StableCell；pending 按稳定 task ID 存 StableBTreeMap，并建立用户、来源交易和待调度索引；迁移使用持久化游标分批推进。MemoryId 只增不复用。给 `my_pending_logs` 增加分页，并限制公开 raw/error 负载。

  验收：以目标最大历史量及 pending 量测量升级、查询和每轮 instructions/内存，迁移可中断续跑且每批受预算约束。证据：无界循环与全量持久化路径；本次未测得生产容量阈值。[ICP stable structures 指南](https://docs.internetcomputer.org/languages/rust/stable-structures/)解释了大规模 pre/post-upgrade 序列化风险。

- [x] **F16 · P2：补齐历史 pending 格式和回滚再升级的兼容性。**

  位置：[store.rs:390](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:390)、[store.rs:651](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:651)。2025-10-17 的 f390b32 之前 pending 没有 fee；BridgeLogLocal.fee 有默认值，但 State.pending 使用的 BridgeLog.fee 没有，直接升级携带旧 pending 的状态会在 load 解码失败。另外，新索引只要非空就跳过旧索引迁移；回滚旧代码期间新增的日志，再升级后不会补到新用户索引。

  动作：明确支持的升级起点，对旧 pending 的 fee 设置历史语义正确的默认/版本迁移；用版本与高水位维护索引，回滚兼容选择双写或再升级补齐，不能把非空当作永久迁移完成。

  验收：真实旧版 State fixture 中含 pending 的升级成功；v1→v2→v1 新增日志→v2 后每个用户历史完整且无重复。两类边界均已通过观察性单元测试复现；当前新格式正常升级不等于触发该旧格式问题。

- [x] **F17 · P2：ATA 存在性检查不要比较整个可变 token account。**

  位置：[svm/rpc.rs:196](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/svm/rpc.rs:196)、[store.rs:3252](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:3252)。只需要判断是否要支付创建 ATA 的租金，却让两家完整 UiAccount.data 相等。两次请求之间收款 ATA 余额变化，双方都确认账户存在也会被判 provider disagreement，正常签名被拒绝。

  动作：增加专用存在性/必要属性读取，在解释返回值后再做保守的布尔/owner 校验；mint 注册验证与 ATA funding 验证使用不同 DTO。验收：同一 ATA 余额从 10 变 11 时不再误拒绝；任意一方报告不存在时预留创建成本。已单元复现。

- [x] **F18 · P2：用结构化错误标记故障链，去掉字符串前缀判断。**

  位置：[store.rs:1646](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:1646)。`err.starts_with(from.name())` 会让 `ETHW: ...` 的故障阻挡 ETH，链名校验允许这两个名字。链状态还依赖错误文本是否恰好带前缀。

  动作：使用 `TaskError { chain, phase, kind, message }`；gate 比较 chain 身份，message 只负责展示。临时最小修复也应比较完整分隔后的链名。验收：ETHW 的 provider 错误不阻挡 ETH；修改错误文案不改变调度/准入行为。证据：可达配置和字符串条件静态确认。

后续性能、简化与运营改进清单：

- [x] **O01：合并相同 RPC，先测 outcall 次数、cycles 和延迟。** 同一轮 Solana `getSignatureStatuses` 可以一次查询多个签名；三项状态的正常双 provider 查询可由 6 次 HTTPS 请求减少到 2 次。共享 finalized block height，保留已有 EVM FinalizeContext 缓存。两家独立 provider 的读取可限并发并行，减少串行等待；不能用降低 F03 的完整性要求换取性能。`two_provider_verdict` 的 AND 判定在已有 false 时也可安全提前返回 false。

- [x] **O02：为 Solana 交易消息加入业务意图 ID，避免相同消息被去重后等待过期重试。** 位置：[store.rs:3340](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/src/one_bridge_canister/src/store.rs:3340)。不同任务相同收款人、金额、payer 和 blockhash 可构造同一 message；Agave 按 message hash 检测 AlreadyProcessed。ICP Ed25519 签名本身是非确定性的，不能推断两次签名必然相等，也不能据此断言重复入账。将稳定 operation ID 编入 Memo 指令，可使不同任务消息不同，同一意图重试仍复用已有签名。验收：同轮两笔等额同收款人出金都能独立成功。参考 [IC Schnorr 规范](https://docs.internetcomputer.org/references/ic-interface-spec/management-canister/#ic-method-sign_with_schnorr)与 [Agave bank 实现](https://github.com/anza-xyz/agave/blob/master/runtime/src/bank.rs)。消息相同已单元复现，未运行真实 Agave 双交易测试。

- [x] **O03：将巨大的 store.rs 按职责拆开，并收紧内部状态表示。** 建议拆成资金意图/状态模型、稳定存储与迁移、调度器、ICRC/EVM/SVM 结算适配器。将 `from_tx`/`to_tx` 的 finalized bool、stuck、error、payout_started_at 等自由组合收束为显式状态；统一 `advance → evidence → commit`，用类型区分暂时失败、终局失败和未知结果。保留 generation/claim 和外部 Candid/稳定编码兼容性。BridgeLogLocal 仅用于稳定存储时可去掉不必要的 CandidType 派生；不能为减少转换行数而直接改变存储短字段或公开接口。

- [x] **O04：为管理员恢复提供可校验的前置条件。** `admin_retry_bridging_task` 目前只拒绝 finalized 出金，允许清除仍可能落链的 to_tx；文档把查链责任交给管理员。ICP 结果未知时 to_tx 还可能为 None，close 的 in-flight 检查也不能表达该状态。增加 expected payout/attempt ID，以及“已终局失败/已可靠过期/人工核账”的恢复依据；未知状态默认保留债权，显式风险操作与普通 retry 分开。重置去重时间前必须证明旧付款不可能成功。验收：过时治理提案或普通 retry 不会抹掉新出现的在途付款。这是运营防误操作增强，不是未授权管理员提权发现。

- [x] **O05：独立核算 ledger 支出与可提现费用，完善配置/初始化校验。** `available_fees` 只减提现金额，而 `icrc1_transfer` 还从桥账户收取 ledger fee；在项目方未另行预充该成本时，全额提走统计手续费可能消耗锁仓准备金。仓库上币规则已要求项目方承担费用，因此这项应落实为可验证的预算与余额预留，不直接认定当前部署资不抵债。固定 ledger/decimals，拒绝匿名治理 principal、明显错误的 fee/min/decimal 组合；`admin_init_public_keys` 失败时返回具体结果，避免用零地址的 Ok 表示成功。验收：提现后仍覆盖待付款和转账费，初始化失败可观测。

- [x] **O06：按用途补 HTTP 认证及缓存。** 当前 `HttpCertification::skip` 是显式设计，README 也有说明，本次不把它列为意外认证绕过。若网页将 HTTP info 的资金地址作为可信输入，应认证固定配置与地址。可按状态版本缓存已脱敏 JSON/CBOR，避免每次复制所有 HashMap。HTTP 边界补 `Accept: application/cbor;q=0`、HEAD 元数据和未知路径语义测试。

- [x] **O07：建立能够覆盖真实 canister 执行模型的验收层。** 原有测试主要覆盖纯函数与 RPC mock，没有实际 upgrade/timer/callback trap/ledger 去重的完整集成验证。增加 PocketIC 的有状态 mock ledger、HTTPS 响应与故障调度场景；将 F02、F04、F08、F10 的一次性资金性质列为必测。稳定状态使用真实历史 fixture；CI 增加 Wasm 构建、Candid 漂移检查、fmt check、严格 lint。固定 Rust 工具链及构建环境；release 当前更新 floating stable，单独固定 ic-wasm/wasm-opt 仍不足以保证将来的字节级可复现构建。

建议实施顺序：F02 的意图模型与 F01 预算先形成共同基础；F03–F08 修正资金证据及上限；随后 F09–F14/F17–F18 修复调度与接口行为；F15–F16 做带 fixture 的存储迁移。O01–O03 的优化与拆分随相应模块修改推进，避免在迁移与付款状态变化之前一次性重写全部引擎。O07 的集成测试应伴随相应修复提交。

审阅覆盖：

| 文件组 | 主要核查内容 |
| --- | --- |
| lib.rs、Cargo.toml、Candid | 模块、依赖、Wasm 构建、接口一致性 |
| api.rs、helper.rs | 身份认证、公开入口、精度、跨 canister 调用语义 |
| api_admin.rs、api_init.rs | 权限、配置并发、初始化、升级、管理恢复、手续费 |
| store.rs | 完整状态机、签名/广播/确认、去重、调度、存储、迁移、全部测试 |
| outcall.rs、evm.rs | 完整性、provider 选择、响应预算、receipt、nonce、gas、ERC-20 ABI、全部测试 |
| svm.rs、svm/rpc.rs、svm/types.rs、svm/spl.rs | Solana finality/expiry、Token-2022、消息和指令编码、ATA、全部测试 |
| ecdsa.rs、schnorr.rs、types.rs | 派生路径、门限签名、地址转换、既有地址兼容测试 |
| api_http.rs | URI、内容协商、响应负载、明确跳过认证的信任限制 |

复现材料：[13 项观察性测试](/Users/zensh/git/github.com/ldclabs/ic-one-bridge/docs/reviews/one_bridge_canister-a2498a8-observations.rs)。测试代码仅追加到临时副本的 store.rs 末尾，生产源文件未改动。复现方式：将基准提交导出到临时目录，将该文件追加到副本 store.rs，运行 `cargo test --locked audit_observations -- --nocapture`。每个测试注释说明它断言的是当前行为；修复后应将断言改为安全行为并纳入对应模块的正式回归测试。

依赖公告扫描的限制：本机 cargo-audit 0.18.3 无法解析最新 RustSec 数据库中的 CVSS 4.0 记录，扫描未完成。错误是 `unsupported CVSS version: 4.0`，发生在加载数据库阶段，不能据此判定本项目依赖是否存在该公告对应的问题。后续应使用支持当前公告格式的工具重新扫描 Cargo.lock。

尚未证明的范围：未进行生产状态审计、真实资产对账、恶意 replica 仿真、实际链重组实验或目标规模的 instructions 基准；第三方依赖源码也没有全部逐行审阅。上述限制不影响已引用源码路径及纯函数/RPC 输入下的复现结论。
