# biliTicker_gt_http

Rust HTTP 服务，支持原有点选/滑块，并内置 GeeTest v4 图标点选。v4 协议、AES/RSA
载荷加密、鼠标轨迹和 ONNX 推理全部在 Rust 进程内完成，不再依赖 Python sidecar。

## GeeTest v4

### 启动

```bash
cargo build --release
sh start.sh
```

Windows 解压 release 包后直接运行：

```bat
start.bat
```

模型位于 `models/`，默认监听 `0.0.0.0:3000`：

- `models/detect.onnx`：快速图标检测器
- `models/detect_icon3.onnx`：低对比度兜底检测器
- `models/simvgg19.onnx`：Siamese 相似度模型

### API

`POST /v4/solve`，兼容旧客户端别名 `POST /api/geetest/solve`：

```json
{
  "captcha_id": "B 站 gee_gt",
  "challenge": "B 站 gee_challenge",
  "risk_type": "icon",
  "max_attempts": 6,
  "proxy": "socks5://127.0.0.1:1080"
}
```

成功响应：

```json
{
  "success": true,
  "data": {
    "lot_number": "...",
    "captcha_output": "...",
    "pass_token": "...",
    "gen_time": "...",
    "attempts": 1,
    "elapsed_ms": 3587.2,
    "candidate_count": 4
  },
  "error": null
}
```

`GET /v4/health` 返回求解成功/失败计数与运行时间。`max_attempts` 默认 6，上限 10；
代理请求失败会回退直连，且同一任务的 `/load`、资源下载、`/verify` 会尽量复用代理出口。

### 性能设计

- 模型启动时加载并预热，请求间复用 ONNX Runtime session。
- 探测/匹配 session 池默认 2 份，服务并发上限默认 4，避免所有请求争抢同一把推理锁。
- 识别先走紧凑检测器与前 2 个排列，失败后才启用 42 MB 兜底检测器和更多候选排列。
- 首次校验等待 1.8 秒，失败重试间隔 1.2 秒，降低极验风控误判。
- 实测一次命中约 3.1–3.7 秒（含 1.8 秒拟人等待）；低对比度挑战通常第 2–3 次通过。

### 配置

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `GT4_MODEL_DIR` | `models` | ONNX 模型目录 |
| `GT4_MAX_CONCURRENCY` | `4` | 同时求解上限，范围 1–4 |
| `GT4_ORT_THREADS` | `2` | 每个 ONNX session 的 CPU 线程数 |

### 模型许可

详见 `models/NOTICE.md`。服务端和模型仅供你有权操作的账号与授权测试使用。

---

## 极验验证模块的python语言绑定

- 以前的[赞助名单](docs/ZANZHU.md)

### 免责声明
- 本项目仅可公益性地用于盲人和老年人等操作验证码困难的人群使用，仅可作为人工过验证码的辅助手段，严禁用于商业目的
- 本项目无法监管下游项目的功能和用途，如果发现下游项目将本项目用于违法犯罪等黑灰产项目，我会立即向有关部门和极验官方举报
- 由于极验尚未提供对残障人士的相关技术服务支持，才有了本项目的诞生。如果极验官方推出相关服务，本项目会及时下架
- 本项目未涉及对服务端加密参数的任何破解，仅涉及普通用户使用正常手段可获取到的代码的逆向分析

### 特别鸣谢
- [Hobr](https://github.com/Hobr) 提供大量技术支持
- [woshiluo](https://github.com/woshiluo) 提供大量技术支持和指导，修复很多本项目的bug
- [mikumifa](https://github.com/mikumifa/biliTickerBuy) 提供部分实现思路

### 使用方式

### Rust 服务调试模式

启动 Rust 服务时传入 `--debug`（或设置环境变量 `BILI_TICKET_GT_DEBUG=1`），会启用详细日志：请求耗时、验证码图片尺寸、模型推理耗时、识别点数量和底层错误。点选与滑块的输入图片会保存到启动目录下的 `debug_artifacts`。

```powershell
cargo run -- --debug
```

调试图片和日志可能包含短时有效的验证码数据，仅用于本地排查；完成后应及时清理 `debug_artifacts`。

1. pip install bili_ticket_gt_python
2. import bili_ticket_gt_python
3. slide = bili_ticket_gt_python.SlidePy()
4. click = bili_ticket_gt_python.ClickPy()
5. 通过slide和click调用相关函数

绑定的函数见py.rs文件
## demo

### 执行测试
```python
import bili_ticket_gt_python
slide = bili_ticket_gt_python.SlidePy()
click = bili_ticket_gt_python.ClickPy()

try:
    #改为注册滑块challenge和gt的url
    validate = slide.test("http://127.0.0.1:5000/pc-geetest/register")
    print(validate)
except Exception as e:
    print("识别失败")
    print(e)

try:
    #改为注册点选challenge和gt的url
    validate = click.test("https://passport.bilibili.com/x/passport-login/captcha?source=main_web")
    print(validate)
except Exception as e:
    print("识别失败")
    print(e)
```

### 通过gt和challenge调用 仅支持点选验证码
```python
import bili_ticket_gt_python
click = bili_ticket_gt_python.ClickPy()
try:
    (gt, challenge) = click.register_test("https://passport.bilibili.com/x/passport-login/captcha?source=main_web")
    validate = click.simple_match(gt, challenge)
    print(validate)
except Exception as e:
    print("识别失败")
    print(e)
```

### 通过gt和challenge调用(自动重试) 仅支持点选验证码
```python
import bili_ticket_gt_python
click = bili_ticket_gt_python.ClickPy()

try:
    (gt, challenge) = click.register_test("https://passport.bilibili.com/x/passport-login/captcha?source=main_web")
    validate = click.simple_match_retry(gt, challenge)
    print(validate)
except Exception as e:
    #验证码失效有两种可能性
    # 1. 验证码生成过后过了太久，时间导致的失效
    # 2. 验证码重试次数过多，需要unset，unset接口请自己扒下来封装吧，我真的懒得封装了
    print("自动重试仍然有极小概率报错(超时时间即为验证码失效时间)")
    print(e)
#不要100%依赖于demo！！！动动脑子
```


### 分步调用
```python
import bili_ticket_gt_python
slide = bili_ticket_gt_python.SlidePy()

try:
    (gt, challenge) = slide.register_test("http://127.0.0.1:5000/pc-geetest/register")
    (_, _) = slide.get_c_s(gt, challenge)
    _type = slide.get_type(gt, challenge)
    if _type != "slide":
        raise Exception("验证码类型错误")
    (c, s, args) = slide.get_new_c_s_args(gt, challenge)
    #注意滑块验证码这里要刷新challenge
    challenge = args[0]
    key = slide.calculate_key(args)
    #rt固定即可
    #此处使用内嵌js二进制运行时生成w, 你也可以自己接入生成w的逻辑函数
    w = slide.generate_w(key, gt, challenge, c, s)
    (msg, validate) = slide.verify(gt, challenge, w)
    print(validate)
except Exception as e:
    print("识别失败")
    print(e)
```

```python
import bili_ticket_gt_python
import time
click = bili_ticket_gt_python.ClickPy()

try:
    (gt, challenge) = click.register_test("https://passport.bilibili.com/x/passport-login/captcha?source=main_web")
    (_, _) = click.get_c_s(gt, challenge)
    _type = click.get_type(gt, challenge)
    if _type != "click":
        raise Exception("验证码类型错误")
    (c, s, args) = click.get_new_c_s_args(gt, challenge)
    before_calculate_key = time.time()
    key = click.calculate_key(args)
    #rt固定即可
    #此处使用内嵌js二进制运行时生成w, 你也可以自己接入生成w的逻辑函数
    w = click.generate_w(key, gt, challenge, c, s)
    #点选验证码生成w后需要等待2秒提交
    w_use_time = time.time() - before_calculate_key
    print("w生成时间：", w_use_time)
    if w_use_time < 2:
        time.sleep(2 - w_use_time)
    (msg, validate) = click.verify(gt, challenge, w)
    print(validate)
except Exception as e:
    print("识别失败")
    print(e)
```
