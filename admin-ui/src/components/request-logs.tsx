import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { listRequests, getRequestDetail } from '@/api/credentials'
import { Card, CardContent } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { RefreshCw, X } from 'lucide-react'
import type { RequestLogSummary, RequestLogDetail } from '@/types/api'

const PAGE_SIZE_OPTIONS = [50, 100, 200, 500]
const DEFAULT_PAGE_SIZE = 100

function formatTs(ms: number): string {
  return new Date(ms).toLocaleString()
}

function formatLatency(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

// 缓存命中展示：undefined=无法判断, 0=未命中, >0=命中(显示估计的缓存读 token)
function cacheCell(cachedTokens?: number) {
  if (cachedTokens == null) return <span className="text-muted-foreground">-</span>
  if (cachedTokens > 0) {
    return <span className="text-green-600" title={`估计缓存读 ${cachedTokens} tokens`}>命中 {cachedTokens.toLocaleString()}</span>
  }
  return <span className="text-muted-foreground">miss</span>
}

function statusBadge(status: string) {
  const variant: 'success' | 'destructive' | 'secondary' = status === 'success'
    ? 'success'
    : status === 'error'
    ? 'destructive'
    : 'secondary'
  return <Badge variant={variant as 'success'}>{status}</Badge>
}

export function RequestLogsPage() {
  const [page, setPage] = useState(0)
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE)
  const [statusFilter, setStatusFilter] = useState<string>('')
  const [accountFilter, setAccountFilter] = useState<string>('')
  const [selectedId, setSelectedId] = useState<string | null>(null)

  const { data, isLoading, refetch, isFetching } = useQuery({
    queryKey: ['requests', page, pageSize, statusFilter, accountFilter],
    queryFn: () => listRequests({
      limit: pageSize,
      offset: page * pageSize,
      status: statusFilter || undefined,
      accountId: accountFilter || undefined,
    }),
    refetchInterval: 5000, // 5s 自动刷新
  })

  const total = data?.total ?? 0
  const items = data?.items ?? []
  const totalPages = Math.ceil(total / pageSize)

  return (
    <div className="space-y-4">
      {/* 工具栏 */}
      <div className="flex flex-wrap items-center gap-3 mb-4">
        <select
          value={statusFilter}
          onChange={(e) => { setStatusFilter(e.target.value); setPage(0) }}
          className="px-3 py-1.5 border rounded text-sm bg-background"
        >
          <option value="">所有状态</option>
          <option value="success">成功</option>
          <option value="error">失败</option>
          <option value="cancelled">取消</option>
        </select>
        <input
          type="text"
          placeholder="按账号 ID 过滤"
          value={accountFilter}
          onChange={(e) => { setAccountFilter(e.target.value); setPage(0) }}
          className="px-3 py-1.5 border rounded text-sm bg-background w-60"
        />
        <select
          value={pageSize}
          onChange={(e) => { setPageSize(Number(e.target.value)); setPage(0) }}
          className="px-3 py-1.5 border rounded text-sm bg-background"
          title="每页条数"
        >
          {PAGE_SIZE_OPTIONS.map((n) => (
            <option key={n} value={n}>每页 {n}</option>
          ))}
        </select>
        <Button variant="outline" size="sm" onClick={() => refetch()} disabled={isFetching}>
          <RefreshCw className={`h-4 w-4 mr-2 ${isFetching ? 'animate-spin' : ''}`} />
          刷新
        </Button>
        <span className="text-sm text-muted-foreground ml-auto">共 {total} 条记录</span>
      </div>

      {/* 列表表格 */}
      <Card>
        <CardContent className="p-0">
          {isLoading ? (
            <div className="p-8 text-center text-muted-foreground">加载中...</div>
          ) : items.length === 0 ? (
            <div className="p-8 text-center text-muted-foreground">
              暂无记录（默认 errors_only 模式只记失败请求；要看全部请改 config.requestLog.mode 为 "all"）
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead className="bg-muted/40 border-b">
                  <tr>
                    <th className="px-3 py-2 text-left font-medium">时间</th>
                    <th className="px-3 py-2 text-left font-medium">状态</th>
                    <th className="px-3 py-2 text-left font-medium">模型</th>
                    <th className="px-3 py-2 text-left font-medium">账号</th>
                    <th className="px-3 py-2 text-right font-medium">耗时</th>
                    <th className="px-3 py-2 text-right font-medium">输入</th>
                    <th className="px-3 py-2 text-right font-medium">输出</th>
                    <th className="px-3 py-2 text-right font-medium">消耗</th>
                    <th className="px-3 py-2 text-right font-medium">上下文%</th>
                    <th className="px-3 py-2 text-center font-medium">缓存</th>
                    <th className="px-3 py-2 text-left font-medium">错误</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((r) => (
                    <RequestRow key={r.requestId} r={r} onClick={() => setSelectedId(r.requestId)} />
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      {/* 分页 */}
      {totalPages > 1 && (
        <div className="flex items-center justify-center gap-3">
          <Button variant="outline" size="sm" onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
            上一页
          </Button>
          <span className="text-sm text-muted-foreground">
            第 {page + 1} / {totalPages} 页
          </span>
          <Button variant="outline" size="sm" onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))} disabled={page >= totalPages - 1}>
            下一页
          </Button>
        </div>
      )}

      {/* 详情侧抽屉 */}
      {selectedId && (
        <RequestDetailDrawer id={selectedId} onClose={() => setSelectedId(null)} />
      )}
    </div>
  )
}

function RequestRow({ r, onClick }: { r: RequestLogSummary; onClick: () => void }) {
  return (
    <tr
      className="border-b hover:bg-muted/30 cursor-pointer"
      onClick={onClick}
    >
      <td className="px-3 py-2 whitespace-nowrap text-xs text-muted-foreground">{formatTs(r.tsMs)}</td>
      <td className="px-3 py-2">{statusBadge(r.status)}</td>
      <td className="px-3 py-2">
        <div className="font-mono text-xs">{r.model}</div>
        <div className="text-xs text-muted-foreground">{r.endpoint}{r.isStream ? ' · 流式' : ''}</div>
      </td>
      <td className="px-3 py-2">
        <div className="text-xs">{r.accountId || '-'}</div>
        {r.accountLabel && <div className="text-xs text-muted-foreground">{r.accountLabel}</div>}
      </td>
      <td className="px-3 py-2 text-right text-xs">
        {formatLatency(r.latencyMs)}
        {r.ttfbMs != null && <div className="text-xs text-muted-foreground">TTFB {formatLatency(r.ttfbMs)}</div>}
      </td>
      <td className="px-3 py-2 text-right text-xs font-mono">{r.promptTokens ?? '-'}</td>
      <td className="px-3 py-2 text-right text-xs font-mono">{r.completionTokens ?? '-'}</td>
      <td className="px-3 py-2 text-right text-xs font-mono">
        {r.meteringUsage != null ? `${r.meteringUsage.toFixed(4)} ${r.meteringUnit ?? ''}` : '-'}
      </td>
      <td className="px-3 py-2 text-right text-xs font-mono">
        {r.contextUsagePct != null ? `${r.contextUsagePct.toFixed(1)}%` : '-'}
      </td>
      <td className="px-3 py-2 text-center text-xs font-mono">{cacheCell(r.cachedTokens)}</td>
      <td className="px-3 py-2 text-xs text-red-500">{r.errorKind || ''}</td>
    </tr>
  )
}

function RequestDetailDrawer({ id, onClose }: { id: string; onClose: () => void }) {
  const { data, isLoading } = useQuery({
    queryKey: ['request-detail', id],
    queryFn: () => getRequestDetail(id),
  })

  return (
    <div
      className="fixed inset-0 z-50 bg-black/40 flex justify-end"
      onClick={onClose}
    >
      <div
        className="bg-background border-l w-full max-w-2xl h-full overflow-y-auto p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold">请求详情</h2>
          <Button variant="ghost" size="icon" onClick={onClose}>
            <X className="h-5 w-5" />
          </Button>
        </div>
        {isLoading ? (
          <div className="text-muted-foreground">加载中...</div>
        ) : data ? (
          <DetailContent data={data} />
        ) : (
          <div className="text-muted-foreground">未找到记录</div>
        )}
      </div>
    </div>
  )
}

function DetailContent({ data }: { data: RequestLogDetail }) {
  return (
    <div className="space-y-4 text-sm">
      <div className="grid grid-cols-2 gap-3">
        <Field label="Request ID" value={data.requestId} mono />
        <Field label="时间" value={formatTs(data.tsMs)} />
        <Field label="状态" value={data.status} />
        <Field label="HTTP" value={String(data.httpStatus ?? '-')} />
        <Field label="账号" value={data.accountId ?? '-'} />
        <Field label="订阅" value={data.accountLabel ?? '-'} />
        <Field label="模型" value={data.model} mono />
        <Field label="上游模型" value={data.upstreamModel ?? '-'} mono />
        <Field label="端点" value={data.endpoint} />
        <Field label="流式" value={data.isStream ? '是' : '否'} />
        <Field label="尝试次数" value={String(data.attempts)} />
        <Field label="结束原因" value={data.reason ?? '-'} />
        <Field label="耗时" value={formatLatency(data.latencyMs)} />
        <Field label="TTFB" value={data.ttfbMs != null ? formatLatency(data.ttfbMs) : '-'} />
        <Field label="输入 tokens" value={String(data.promptTokens ?? '-')} />
        <Field label="输出 tokens" value={String(data.completionTokens ?? '-')} />
        <Field label="消耗" value={data.meteringUsage != null ? `${data.meteringUsage.toFixed(4)} ${data.meteringUnit ?? ''}` : '-'} />
        <Field label="缓存命中" value={data.cachedTokens == null ? '-' : data.cachedTokens > 0 ? `命中 (缓存读 ~${data.cachedTokens.toLocaleString()})` : '未命中'} />
        <Field label="上下文使用" value={data.contextUsagePct != null ? `${data.contextUsagePct.toFixed(1)}%` : '-'} />
        <Field label="messages 数" value={String(data.messagesCount ?? '-')} />
        <Field label="tools 数" value={String(data.toolsCount ?? '-')} />
        <Field label="system 长度" value={String(data.systemPromptLen ?? '-')} />
        <Field label="cache_control" value={data.hasCacheControl === true ? '是' : data.hasCacheControl === false ? '否' : '-'} />
      </div>

      {data.errorKind && (
        <div className="border-t pt-3">
          <div className="font-medium text-red-500 mb-2">错误</div>
          <Field label="kind" value={data.errorKind} />
          <Field label="stage" value={data.errorStage ?? '-'} />
          <div className="mt-2">
            <div className="text-xs text-muted-foreground mb-1">message</div>
            <pre className="bg-muted p-2 rounded text-xs overflow-x-auto whitespace-pre-wrap">{data.errorMessage}</pre>
          </div>
        </div>
      )}

      {data.requestBody && (
        <div className="border-t pt-3">
          <div className="font-medium mb-2">请求体（Kiro 格式）</div>
          <pre className="bg-muted p-2 rounded text-xs overflow-x-auto max-h-96 whitespace-pre-wrap">{prettyJson(data.requestBody)}</pre>
        </div>
      )}

      {data.responseBody && (
        <div className="border-t pt-3">
          <div className="font-medium mb-2">响应体</div>
          <pre className="bg-muted p-2 rounded text-xs overflow-x-auto max-h-96 whitespace-pre-wrap">{prettyJson(data.responseBody)}</pre>
        </div>
      )}
    </div>
  )
}

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className={`text-sm ${mono ? 'font-mono break-all' : ''}`}>{value}</div>
    </div>
  )
}

function prettyJson(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2)
  } catch {
    return s
  }
}
