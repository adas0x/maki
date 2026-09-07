-- Luau is Lua plus types, and the luau grammar keeps the node shapes lua_lang
-- relies on, so require and const handling stays there and this file adds only
-- what the type system brings.
return function(U)
  local base = require("lang.lua_lang")(U)
  local get_text = U.get_text
  local find_child = U.find_child
  local compact_ws = U.compact_ws
  local truncate = U.truncate
  local new_entry = U.new_entry
  local SECTION = U.SECTION

  local extract_lua_nodes = base.extract_nodes

  local function is_constant_name(name)
    for seg in name:gmatch("[^%.]+") do
      if not seg:match("^%u") then
        return false
      end
    end
    return true
  end

  -- Covers both assignment_statement and variable_declaration: the var list's
  -- first child is the name in both, and type annotations or <const> trail it
  -- as extra children where lua_lang's single-name check gives up.
  local function constant_entry(node, source)
    local assign = find_child(node, "assignment_statement") or node
    local var_list = find_child(assign, "variable_list")
    local expr_list = find_child(assign, "expression_list")
    if not var_list or not expr_list then
      return nil
    end
    local name_node, commas = nil, 0
    for i = 0, var_list:child_count() - 1 do
      local child = var_list:child(i)
      if child:type() == "," then
        commas = commas + 1
      elseif not name_node then
        name_node = child
      end
    end
    local name = name_node and get_text(name_node, source) or ""
    if commas > 0 or not is_constant_name(name) then
      return nil
    end
    local val = expr_list:named_child(0)
    if not val then
      return nil
    end
    if val:type() == "function_definition" then
      local params = val:field("parameters")[1]
      local label = name .. (params and get_text(params, source) or "()")
      return new_entry(SECTION.Function, node, compact_ws(label))
    end
    local val_str = " = " .. truncate(compact_ws(get_text(val, source)), 60)
    return new_entry(SECTION.Constant, node, name .. val_str)
  end

  local function function_entry(node, source)
    local name_node = node:field("name")[1]
    if not name_node then
      return nil
    end
    local label = get_text(name_node, source)
    local generics = find_child(node, "generic_type_list")
    if generics then
      label = label .. get_text(generics, source)
    end
    local params_node = node:field("parameters")[1]
    label = label .. (params_node and get_text(params_node, source) or "()")
    -- The grammar has no field for the return type; it sits between the
    -- parameters and the body, so the body's previous named sibling is it.
    local body_node = node:field("body")[1]
    local ret_node = body_node and body_node:prev_named_sibling()
    if ret_node and ret_node:type() ~= "parameters" then
      label = label .. ": " .. get_text(ret_node, source)
    end
    return new_entry(SECTION.Function, node, compact_ws(label))
  end

  local function type_entry(node, source)
    return new_entry(SECTION.Type, node, truncate(compact_ws(get_text(node, source)), 80))
  end

  base.extract_nodes = function(node, source, attrs)
    local kind = node:type()

    if kind == "function_declaration" then
      local e = function_entry(node, source)
      return e and { e } or {}
    elseif kind == "type_definition" then
      return { type_entry(node, source) }
    elseif kind == "assignment_statement" then
      local e = constant_entry(node, source)
      return e and { e } or {}
    elseif kind == "variable_declaration" then
      -- Imports and untyped consts already work in lua_lang; typed and
      -- <const> locals fall through to here because its name check fails.
      local extracted = extract_lua_nodes(node, source, attrs)
      if #extracted > 0 then
        return extracted
      end
      local e = constant_entry(node, source)
      return e and { e } or {}
    end

    return extract_lua_nodes(node, source, attrs)
  end

  return base
end
