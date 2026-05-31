import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getCredentials,
  setCredentialDisabled,
  setCredentialPriority,
  resetCredentialFailure,
  forceRefreshToken,
  getCredentialBalance,
  addCredential,
  deleteCredential,
  getLoadBalancingMode,
  setLoadBalancingMode,
  getRateLimitCooldown,
  setRateLimitCooldown,
  getScheduling,
  updateScheduling,
  setCredentialConcurrency,
  listApiKeys,
  addApiKey,
  deleteApiKey,
  setApiKeyDisabled,
  listGroups,
  addGroup,
  renameGroup,
  deleteGroup,
  setCredentialGroup,
  setApiKeyGroup,
} from '@/api/credentials'
import type { AddCredentialRequest } from '@/types/api'

// 查询凭据列表
export function useCredentials() {
  return useQuery({
    queryKey: ['credentials'],
    queryFn: getCredentials,
    refetchInterval: 30000, // 每 30 秒刷新一次
  })
}

// 查询凭据余额
export function useCredentialBalance(id: number | null) {
  return useQuery({
    queryKey: ['credential-balance', id],
    queryFn: () => getCredentialBalance(id!),
    enabled: id !== null,
    retry: false, // 余额查询失败时不重试（避免重复请求被封禁的账号）
  })
}

// 设置禁用状态
export function useSetDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, disabled }: { id: number; disabled: boolean }) =>
      setCredentialDisabled(id, disabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 设置优先级
export function useSetPriority() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, priority }: { id: number; priority: number }) =>
      setCredentialPriority(id, priority),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 重置失败计数
export function useResetFailure() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => resetCredentialFailure(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 强制刷新 Token
export function useForceRefreshToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => forceRefreshToken(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 添加新凭据
export function useAddCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: AddCredentialRequest) => addCredential(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 删除凭据
export function useDeleteCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteCredential(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 获取负载均衡模式
export function useLoadBalancingMode() {
  return useQuery({
    queryKey: ['loadBalancingMode'],
    queryFn: getLoadBalancingMode,
  })
}

// 设置负载均衡模式
export function useSetLoadBalancingMode() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setLoadBalancingMode,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['loadBalancingMode'] })
    },
  })
}

// 获取限流冷却时长
export function useRateLimitCooldown() {
  return useQuery({
    queryKey: ['rateLimitCooldown'],
    queryFn: getRateLimitCooldown,
  })
}

// 设置限流冷却时长
export function useSetRateLimitCooldown() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setRateLimitCooldown,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['rateLimitCooldown'] })
    },
  })
}

// 获取调度策略（模式 + 冷却 + 会话亲和参数）
export function useScheduling() {
  return useQuery({
    queryKey: ['scheduling'],
    queryFn: getScheduling,
  })
}

// 更新调度策略（仅传需要改的字段）
export function useUpdateScheduling() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: updateScheduling,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['scheduling'] })
    },
  })
}

// 列出反代访问密钥（脱敏）
export function useApiKeys() {
  return useQuery({
    queryKey: ['apiKeys'],
    queryFn: listApiKeys,
  })
}

// 新增反代访问密钥
export function useAddApiKey() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ key, label }: { key: string; label?: string }) => addApiKey(key, label),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['apiKeys'] })
    },
  })
}

// 删除反代访问密钥
export function useDeleteApiKey() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteApiKey(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['apiKeys'] })
    },
  })
}

// 设置反代访问密钥的启用/禁用状态
export function useSetApiKeyDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, disabled }: { id: number; disabled: boolean }) =>
      setApiKeyDisabled(id, disabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['apiKeys'] })
    },
  })
}

// 设置凭据并发上限
export function useSetConcurrency() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, maxConcurrency }: { id: number; maxConcurrency: number }) =>
      setCredentialConcurrency(id, maxConcurrency),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// ============ 账号池分组 ============

// 列出全部分组（含成员凭据 id）
export function useGroups() {
  return useQuery({
    queryKey: ['groups'],
    queryFn: listGroups,
  })
}

// 新建分组
export function useAddGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => addGroup(name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['groups'] })
    },
  })
}

// 重命名分组
export function useRenameGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, name }: { id: number; name: string }) => renameGroup(id, name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['groups'] })
    },
  })
}

// 删除分组（级联清空归属、解绑 apikey）
export function useDeleteGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteGroup(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['groups'] })
      queryClient.invalidateQueries({ queryKey: ['apiKeys'] })
    },
  })
}

// 设置某凭据的分组归属
export function useSetCredentialGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, groupId }: { id: number; groupId: number | null }) =>
      setCredentialGroup(id, groupId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['groups'] })
    },
  })
}

// 设置某 apikey 的分组绑定
export function useSetApiKeyGroup() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, groupId }: { id: number; groupId: number | null }) =>
      setApiKeyGroup(id, groupId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['apiKeys'] })
    },
  })
}
