import { useState } from 'react'
import { Trash2, Plus, Check, X, Pencil } from 'lucide-react'
import { toast } from 'sonner'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import {
  useGroups,
  useAddGroup,
  useRenameGroup,
  useDeleteGroup,
  useCredentials,
  useSetCredentialGroup,
} from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'

/**
 * 账号池分组管理面板
 *
 * - 建组 / 改名 / 删组
 * - 把账号（凭据）归入分组：每个账号一个下拉，选所属分组
 * - apikey 绑定分组在 API Key 列表里每行的下拉（见 api-keys-panel）
 *
 * 分组路由语义（fail-open）：apikey 绑定分组后优先用该分组内的账号；
 * 组内账号全挂时回退到全部账号，优先保证可用而非硬隔离。
 */
export function GroupsPanel() {
  const { data: groupsData, isLoading } = useGroups()
  const { data: credsData } = useCredentials()
  const { mutate: addGroup, isPending: isAdding } = useAddGroup()
  const { mutate: renameGroup } = useRenameGroup()
  const { mutate: deleteGroup } = useDeleteGroup()
  const { mutate: setCredGroup } = useSetCredentialGroup()

  const [newName, setNewName] = useState('')
  const [editingId, setEditingId] = useState<number | null>(null)
  const [editName, setEditName] = useState('')

  const groups = groupsData?.groups ?? []
  const creds = credsData?.credentials ?? []

  // credential_id → group_id 反查（用于每个账号下拉的当前值）
  const credGroupOf = new Map<number, number>()
  for (const g of groups) {
    for (const cid of g.credentialIds) credGroupOf.set(cid, g.id)
  }

  const handleAdd = () => {
    const name = newName.trim()
    if (!name) {
      toast.error('请输入分组名')
      return
    }
    addGroup(name, {
      onSuccess: () => {
        toast.success('分组已创建')
        setNewName('')
      },
      onError: (error) => toast.error(`创建失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleRename = (id: number) => {
    const name = editName.trim()
    if (!name) {
      toast.error('分组名不能为空')
      return
    }
    renameGroup(
      { id, name },
      {
        onSuccess: () => {
          toast.success('分组已重命名')
          setEditingId(null)
        },
        onError: (error) => toast.error(`重命名失败: ${extractErrorMessage(error)}`),
      },
    )
  }

  const handleDelete = (id: number, name: string) => {
    if (!confirm(`确定删除分组「${name}」？绑定它的 API Key 会被解绑（回退为可用全部账号）。`)) return
    deleteGroup(id, {
      onSuccess: () => toast.success('分组已删除'),
      onError: (error) => toast.error(`删除失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleSetCredGroup = (credId: number, groupId: number | null) => {
    setCredGroup(
      { id: credId, groupId },
      {
        onSuccess: () => toast.success('账号分组已更新'),
        onError: (error) => toast.error(`更新失败: ${extractErrorMessage(error)}`),
      },
    )
  }

  const credLabel = (c: { id: number; email?: string }) =>
    c.email ? `#${c.id} ${c.email}` : `#${c.id}`

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">
        把账号分到不同分组，API Key 绑定分组后优先用该组内的账号（分组路由）。组内账号全挂时回退到全部账号，保证可用（fail-open）。
      </p>

      {isLoading ? (
        <div className="text-sm text-muted-foreground py-2 text-center">加载中...</div>
      ) : (
        <div className="space-y-3">
          {/* 分组列表 */}
          <div className="space-y-2">
            {groups.length === 0 ? (
              <div className="text-xs text-muted-foreground py-2 text-center">
                暂无分组，新建一个开始
              </div>
            ) : (
              groups.map((g) => (
                <div key={g.id} className="flex items-center justify-between gap-2 rounded-md border px-3 py-2">
                  {editingId === g.id ? (
                    <div className="flex items-center gap-1 flex-1 min-w-0">
                      <Input
                        value={editName}
                        onChange={(e) => setEditName(e.target.value)}
                        className="h-7 text-sm"
                        autoFocus
                      />
                      <Button variant="ghost" size="icon" className="h-7 w-7 text-green-600" onClick={() => handleRename(g.id)}>
                        <Check className="h-4 w-4" />
                      </Button>
                      <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => setEditingId(null)}>
                        <X className="h-4 w-4" />
                      </Button>
                    </div>
                  ) : (
                    <>
                      <div className="min-w-0">
                        <div className="text-sm font-medium truncate">{g.name}</div>
                        <div className="text-xs text-muted-foreground">
                          {g.credentialIds.length} 个账号
                        </div>
                      </div>
                      <div className="flex items-center gap-1 shrink-0">
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          onClick={() => {
                            setEditingId(g.id)
                            setEditName(g.name)
                          }}
                          title="重命名"
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7 text-destructive hover:text-destructive"
                          onClick={() => handleDelete(g.id, g.name)}
                          title="删除分组"
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </>
                  )}
                </div>
              ))
            )}
          </div>

          {/* 新建分组 */}
          <div className="flex items-center gap-2">
            <Input
              type="text"
              placeholder="新分组名"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
              className="h-8"
            />
            <Button onClick={handleAdd} disabled={isAdding || !newName.trim()} size="sm" className="shrink-0">
              <Plus className="h-4 w-4 mr-1" />
              建组
            </Button>
          </div>

          {/* 账号归组 */}
          {groups.length > 0 && creds.length > 0 && (
            <div className="space-y-2 border-t pt-3">
              <span className="text-xs text-muted-foreground">账号归组</span>
              {creds.map((c) => (
                <div key={c.id} className="flex items-center justify-between gap-2">
                  <span className="text-xs font-mono truncate min-w-0 flex-1" title={credLabel(c)}>
                    {credLabel(c)}
                  </span>
                  <select
                    value={credGroupOf.get(c.id) ?? ''}
                    onChange={(e) =>
                      handleSetCredGroup(c.id, e.target.value === '' ? null : Number(e.target.value))
                    }
                    className="h-7 w-40 rounded border border-input bg-transparent px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring shrink-0"
                  >
                    <option value="">未分组</option>
                    {groups.map((g) => (
                      <option key={g.id} value={g.id}>
                        {g.name}
                      </option>
                    ))}
                  </select>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
