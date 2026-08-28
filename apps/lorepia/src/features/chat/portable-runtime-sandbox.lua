local runtime_original_pcall = pcall
local runtime_original_resume = coroutine.resume
local runtime_error = error
local runtime_tostring = tostring
local runtime_string_find = string.find
local runtime_string_lower = string.lower
local runtime_table_pack = table.pack
local runtime_table_unpack = table.unpack

local function runtime_is_timeout(value)
    local message = runtime_string_lower(runtime_tostring(value))
    return runtime_string_find(message, "timeout", 1, true) ~= nil
end

local function runtime_unpack_checked(results)
    if results[1] == false and runtime_is_timeout(results[2]) then
        runtime_error(results[2], 0)
    end
    return runtime_table_unpack(results, 1, results.n)
end

pcall = function(callback, ...)
    return runtime_unpack_checked(runtime_table_pack(runtime_original_pcall(callback, ...)))
end

xpcall = function(callback, handler, ...)
    local called = runtime_table_pack(runtime_original_pcall(callback, ...))
    if called[1] then
        return runtime_table_unpack(called, 1, called.n)
    end
    if runtime_is_timeout(called[2]) then
        runtime_error(called[2], 0)
    end

    local handled = runtime_table_pack(runtime_original_pcall(handler, called[2]))
    if handled[1] == false then
        if runtime_is_timeout(handled[2]) then
            runtime_error(handled[2], 0)
        end
        return false, handled[2]
    end
    return false, handled[2]
end

coroutine.resume = function(thread, ...)
    return runtime_unpack_checked(
        runtime_table_pack(runtime_original_resume(thread, ...))
    )
end

debug = nil
package = nil
io = nil
os = nil
load = nil
loadfile = nil
dofile = nil
require = nil
loadstring = nil
Promise = nil
__hostIsRuntimePromise = nil
__hostRuntimeYield = nil
