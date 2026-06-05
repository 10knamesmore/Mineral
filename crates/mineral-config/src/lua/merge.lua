-- deep_merge(base, override):返回新表;override 的键覆盖 base。
-- 两侧同键且都是「map 表」→ 递归深合并;否则(标量 / 数组 / 类型不一)→ override 整体替换。
-- 数组判定:序列表(连续整数键从 1 起)视为数组,整体替换不逐元素合并(设计 D3)。

-- 连续整数键检测;空表视为非数组(空 override 对 map base = 不改变,保 merge(d,{})==d)。
local function is_array(t)
    local n = 0
    for _ in pairs(t) do
        n = n + 1
    end
    if n == 0 then
        return false
    end
    for i = 1, n do
        if t[i] == nil then
            return false
        end
    end
    return true
end

-- map 表 = table 且非数组。
local function is_map(v)
    return type(v) == "table" and not is_array(v)
end

local function deep_merge(base, override)
    -- 两侧都是 map 才递归;否则 override 整体替换(标量 / 数组 / 类型不一)。
    if not (is_map(base) and is_map(override)) then
        return override
    end
    local result = {}
    for k, v in pairs(base) do
        result[k] = v
    end
    for k, v in pairs(override) do
        if is_map(result[k]) and is_map(v) then
            result[k] = deep_merge(result[k], v)
        else
            result[k] = v
        end
    end
    return result
end

return deep_merge
