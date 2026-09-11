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

  -- Name, generics, parameters and the return type of any function-shaped
  -- node. The return type has no field: it sits before the body when there
  -- is one, otherwise it is the last named child.
  local function signature_label(node, source)
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
    local body_node = node:field("body")[1]
    local ret_node = body_node and body_node:prev_named_sibling()
    if not ret_node then
      local count = node:named_child_count()
      ret_node = count > 0 and node:named_child(count - 1)
    end
    if ret_node and ret_node:type() ~= "parameters" and ret_node:type() ~= "generic_type_list" then
      label = label .. ": " .. get_text(ret_node, source)
    end
    return compact_ws(label)
  end

  local function function_entry(node, source)
    local label = signature_label(node, source)
    return label and new_entry(SECTION.Function, node, label) or nil
  end

  local function type_entry(node, source)
    return new_entry(SECTION.Type, node, truncate(compact_ws(get_text(node, source)), 80))
  end

  local function type_function_entry(node, source)
    local label = signature_label(node, source)
    if not label then
      return nil
    end
    local prefix = "type function "
    local first = node:child(0)
    if first and first:type() == "export" then
      prefix = "export " .. prefix
    end
    return new_entry(SECTION.Type, node, prefix .. label)
  end

  local function declare_type_entry(node, source)
    local kind_node = node:child(1)
    local label = kind_node and kind_node:type() == "extern" and "extern type" or "class"
    local name_node = node:field("name")[1]
    label = label .. " " .. (name_node and get_text(name_node, source) or "?")
    local super_node = node:field("super")[1]
    if super_node then
      label = label .. " extends " .. get_text(super_node, source)
    end
    local entry = new_entry(SECTION.Type, node, label)
    local members = {}
    for _, child in ipairs(node:named_children()) do
      if child:type() == "declare_member" then
        members[#members + 1] = compact_ws(get_text(child, source))
      end
    end
    entry.children = members
    return entry
  end

  base.extract_nodes = function(node, source, attrs)
    local kind = node:type()

    if kind == "function_declaration" then
      local e = function_entry(node, source)
      return e and { e } or {}
    elseif kind == "type_definition" then
      return { type_entry(node, source) }
    elseif kind == "type_function" then
      local e = type_function_entry(node, source)
      return e and { e } or {}
    elseif kind == "assignment_statement" then
      local e = constant_entry(node, source)
      return e and { e } or {}
    elseif kind == "const_statement" then
      if node:field("parameters")[1] then
        local e = function_entry(node, source)
        return e and { e } or {}
      end
      local e = constant_entry(node, source)
      return e and { e } or {}
    elseif kind == "declare_statement" then
      if node:field("parameters")[1] then
        local e = function_entry(node, source)
        return e and { e } or {}
      end
      local type_node = node:field("type")[1]
      if type_node then
        local name_node = node:field("name")[1]
        local name = name_node and get_text(name_node, source) or ""
        return { new_entry(SECTION.Constant, node, name .. ": " .. compact_ws(get_text(type_node, source))) }
      end
      local e = declare_type_entry(node, source)
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
