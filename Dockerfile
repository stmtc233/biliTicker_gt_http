# 使用最小的基础镜像
FROM debian:bookworm-slim

# 安装运行时依赖
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# 创建非 root 用户
RUN groupadd -r appuser && useradd -r -g appuser appuser

# 设置工作目录
WORKDIR /app

# 复制编译好的二进制文件
COPY bili-ticket-gt-server-linux-x86_64 /app/bili-ticket-gt-server

# 设置权限
RUN chmod +x /app/bili-ticket-gt-server && \
    chown appuser:appuser /app/bili-ticket-gt-server

# 切换到非 root 用户
USER appuser

# 暴露端口
EXPOSE 3000

# 设置健康检查
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

# 启动应用
CMD ["./bili-ticket-gt-server"]