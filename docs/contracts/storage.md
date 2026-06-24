# Roze 对象存储契约

`roze-storage` 提供统一对象存储接口，用于图片、附件、头像、导入文件等上传场景。

## Provider

支持的配置边界：

- `local`
- `s3_compatible`
- `qiniu_kodo`
- `aliyun_oss`
- `tencent_cos`

当前已完整可执行：

- `local`：本地落盘、读取、删除、stat、presign URL。

当前已提供统一配置和接口边界：

- 七牛 Kodo
- 阿里云 OSS
- 腾讯云 COS
- S3 API providers

云厂商真实签名上传需要继续接具体 SDK 或签名协议；业务代码先依赖 `ObjectStorage` trait，不直接依赖厂商 SDK。

## 配置示例

```yaml
storage:
  provider: aliyun_oss
  bucket: images
  endpoint: https://oss-cn-hangzhou.aliyuncs.com
  region: cn-hangzhou
  access_key: ${OSS_ACCESS_KEY}
  secret_key: ${OSS_SECRET_KEY}
  public_base_url: https://cdn.example.com
  tenant_prefix: tenant-a
  validation:
    max_size_bytes: 10485760
    allowed_mime_types: [image/jpeg, image/png, image/webp, image/gif]
    allowed_extensions: [jpg, jpeg, png, webp, gif]
```

## 统一接口

`ObjectStorage`：

- `put_object`
- `get_object`
- `delete_object`
- `stat_object`
- `presign_put`
- `presign_get`

`PutObjectRequest`：

- `key`
- `bytes`
- `content_type`
- `metadata`

`ObjectInfo`：

- `provider`
- `bucket`
- `key`
- `size`
- `content_type`
- `etag`
- `url`
- `metadata`
- `updated_at_millis`

## 安全边界

已实现：

- key 规范化。
- 阻止 `/` 开头、空 key、`\0`、`..` 路径穿越。
- 非安全 path segment 自动转 `_`。
- 默认图片 MIME 白名单。
- 默认图片扩展名白名单。
- 默认 10MB 上传上限。
- tenant prefix。

后续生产增强：

- 七牛/OSS/COS 真实服务端签名。
- 分片上传。
- 回调验签。
- 图片尺寸/内容探测。
- 病毒扫描接口。
- CDN 刷新/预热。
- lifecycle/归档策略。
