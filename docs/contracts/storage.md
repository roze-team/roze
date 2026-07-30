# Roze 对象存储契约

`roze-storage` 为图片、附件、头像和导入文件提供统一的 `ObjectStorage`
接口。业务模块只依赖该接口，不直接依赖云厂商 SDK。

## Provider

- `local`：本地读写、删除、stat 和 URL 生成。
- `s3_compatible`：基于 AWS Signature V4 的 S3 兼容运行时。
- `qiniu_kodo`：复用同一 SigV4 运行时，通过七牛 Kodo 官方 S3
  兼容 endpoint 执行服务端读写和预签名。
- `aliyun_oss`、`tencent_cos`：保留配置边界；在专用签名适配器完成前，
  mutation 继续 fail closed。

Kodo 配置示例：

```yaml
storage:
  provider: qiniu_kodo
  bucket: roze-images
  endpoint: https://s3.cn-east-1.qiniucs.com
  region: cn-east-1
  access_key: ${QINIU_ACCESS_KEY}
  secret_key: ${QINIU_SECRET_KEY}
  public_base_url: https://media.example.com
  tenant_prefix: tenant-a
  validation:
    max_size_bytes: 10485760
    allowed_mime_types: [image/jpeg, image/png, image/webp, image/gif]
    allowed_extensions: [jpg, jpeg, png, webp, gif]
```

`bucket` 必须填写七牛空间对应的 S3 空间名。`endpoint` 和 `region`
必须属于同一区域。

## 统一接口

- `put_object`
- `get_object`
- `delete_object`
- `stat_object`
- `presign_put`
- `presign_get`

服务端上传统一执行大小、MIME、扩展名、对象键和 tenant prefix 校验。
预签名上传只签发短期 PUT URL；调用方仍应在上传完成后通过 `stat_object`
校验对象，并由业务层决定失败清理和覆盖策略。

## 安全边界

- 拒绝空 key、绝对路径、NUL 和 `..` 路径穿越。
- 默认图片 MIME/扩展名白名单和 10 MiB 上限。
- AK/SK 在 `Debug` 输出中始终脱敏。
- Kodo 使用官方 S3 兼容 endpoint 与 AWS Signature V4，不生成无签名的
  “兼容 URL”。
- 七牛原生上传策略、回调验签、分片上传、CDN 刷新和内容安全不属于当前
  S3 兼容适配器；需要这些能力时应作为独立 provider 扩展实现。
