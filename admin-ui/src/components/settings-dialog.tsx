import { useState } from 'react'
import { Trash2, Plus, Power, PowerOff } from 'lucide-react'
import { toast } from 'sonner'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import {
  useApiKeys,
  useAddApiKey,
  useDeleteApiKey,
  useSetApiKeyDisabled,
  useGroups,
  useSetApiKeyGroup,
} from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import { SchedulingPanel } from '@/components/scheduling-panel'
import { GroupsPanel } from '@/components/groups-panel'

interface SettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function SettingsDialog({ open, onOpenChange }: SettingsDialogProps) {
  const { data, isLoading } = useApiKeys()
  const { mutate: addKey, isPending: isAdding } = useAddApiKey()
  const { mutate: deleteKey } = useDeleteApiKey()
  const { mutate: toggleDisabled } = useSetApiKeyDisabled()
  const { data: groupsData } = useGroups()
  const { mutate: setKeyGroup } = useSetApiKeyGroup()
  const [newKey, setNewKey] = useState('')
  const [newLabel, setNewLabel] = useState('')

  const keys = data?.keys ?? []
  const groups = groupsData?.groups ?? []
  const enabledCount = keys.filter((k) => !k.disabled).length

  const handleSetKeyGroup = (id: number, groupId: number | null) => {
    setKeyGroup(
      { id, groupId },
      {
        onSuccess: () => toast.success('分组绑定已更新'),
        onError: (error) => toast.error(`更新失败: ${extractErrorMessage(error)}`),
      },
    )
  }

  const handleAdd = () => {
    const trimmed = newKey.trim()
    if (!trimmed) {
      toast.error('请输入新的 API Key')
      return
    }
    addKey(
      { key: trimmed, label: newLabel.trim() || undefined },
      {
        onSuccess: () => {
          toast.success('API Key 已添加')
          setNewKey('')
          setNewLabel('')
        },
        onError: (error) => toast.error(`添加失败: ${extractErrorMessage(error)}`),
      },
    )
  }

  const handleDelete = (id: number) => {
    if (!confirm('确定删除这个 API Key？使用它的客户端会立即失效，且无法恢复。')) return
    deleteKey(id, {
      onSuccess: () => toast.success('API Key 已删除'),
      onError: (error) => toast.error(`删除失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleToggle = (id: number, currentlyDisabled: boolean) => {
    const action = currentlyDisabled ? '启用' : '禁用'
    if (!confirm(`确定${action}这个 API Key？${currentlyDisabled ? '' : '使用它的客户端会立即认证失败。'}`)) {
      return
    }
    toggleDisabled(
      { id, disabled: !currentlyDisabled },
      {
        onSuccess: () => toast.success(`API Key 已${action}`),
        onError: (error) => toast.error(`${action}失败: ${extractErrorMessage(error)}`),
      },
    )
  }

  const formatDate = (ms: number) => new Date(ms).toLocaleString('zh-CN')

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg max-h-[85vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>反代 API Key</DialogTitle>
          <DialogDescription>
            客户端调用 /v1/messages 的访问密钥，可配置多个，任意一个均可通过认证。增删立即生效并持久化到 SQLite。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* 现有 key 列表 */}
          <div className="space-y-2">
            {isLoading ? (
              <div className="text-sm text-muted-foreground py-4 text-center">加载中...</div>
            ) : keys.length === 0 ? (
              <div className="text-sm text-muted-foreground py-4 text-center">暂无 API Key</div>
            ) : (
              keys.map((k) => {
                const isLastEnabled = !k.disabled && enabledCount <= 1
                return (
                  <div
                    key={k.id}
                    className={`flex items-center justify-between gap-2 rounded-md border px-3 py-2 ${
                      k.disabled ? 'bg-muted/40 opacity-60' : ''
                    }`}
                  >
                    <div className="min-w-0">
                      <div className="font-mono text-sm truncate flex items-center gap-2">
                        {k.masked}
                        {k.disabled && (
                          <span className="text-[10px] uppercase tracking-wide bg-destructive/20 text-destructive px-1.5 py-0.5 rounded">
                            已禁用
                          </span>
                        )}
                      </div>
                      <div className="text-xs text-muted-foreground truncate">
                        {k.label ? `${k.label} · ` : ''}
                        {formatDate(k.createdAt)}
                      </div>
                      <select
                        value={k.groupId ?? ''}
                        onChange={(e) =>
                          handleSetKeyGroup(k.id, e.target.value === '' ? null : Number(e.target.value))
                        }
                        className="mt-1 h-7 w-full rounded border border-input bg-transparent px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        title="该 Key 可用的账号分组（严格隔离）"
                      >
                        <option value="">未分组（全部账号）</option>
                        {groups.map((g) => (
                          <option key={g.id} value={g.id}>
                            分组：{g.name}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="flex items-center gap-1 shrink-0">
                      <Button
                        variant="ghost"
                        size="icon"
                        className={k.disabled ? 'text-green-600 hover:text-green-700' : 'text-amber-600 hover:text-amber-700'}
                        onClick={() => handleToggle(k.id, k.disabled)}
                        disabled={isLastEnabled}
                        title={
                          isLastEnabled
                            ? '至少保留一个启用的 API Key'
                            : k.disabled
                              ? '启用'
                              : '禁用'
                        }
                      >
                        {k.disabled ? <Power className="h-4 w-4" /> : <PowerOff className="h-4 w-4" />}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="text-destructive hover:text-destructive"
                        onClick={() => handleDelete(k.id)}
                        disabled={keys.length <= 1}
                        title={keys.length <= 1 ? '至少保留一个 API Key' : '删除'}
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                )
              })
            )}
          </div>

          {/* 新增 key */}
          <div className="space-y-2 border-t pt-4">
            <span className="text-sm text-muted-foreground">新增 Key</span>
            <Input
              type="text"
              placeholder="API Key"
              value={newKey}
              onChange={(e) => setNewKey(e.target.value)}
              autoComplete="off"
            />
            <Input
              type="text"
              placeholder="备注（可选）"
              value={newLabel}
              onChange={(e) => setNewLabel(e.target.value)}
              autoComplete="off"
            />
            <Button onClick={handleAdd} disabled={isAdding || !newKey.trim()} className="w-full">
              <Plus className="h-4 w-4 mr-2" />
              {isAdding ? '添加中...' : '添加'}
            </Button>
          </div>

          {/* 账号池分组 */}
          <GroupsPanel />

          {/* 调度策略 */}
          <SchedulingPanel />
        </div>
      </DialogContent>
    </Dialog>
  )
}
