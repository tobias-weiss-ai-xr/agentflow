import json, re

with open('config/lw-tasks.json', 'rb') as f:
    raw = f.read()

# read as text, stripping BOM and coupling control chars
 text = raw.decode('utf-8-sig')
# First, replace literal nulls
text = text.replace('\x00', '')
#JSON strings may contain raw newlines from multi-line fields in the original
# We will fix by parsing with relaxed controlChar requirements, but Python json won't allow
# So instead split into lines and locate+escape control chars
# Actually, erroneous lines: replace any raw \r or \n *inside string values* with JSON escapes
# We do it line-based: if a line starts with space+quote it's inside a string -> replace \r and \n with \r and \n
lines=text.split('\n')
fixed=[]
in_string=False
for line in lines:
    if not in_string and '\"' in line:
        # crude heuristic: find first " after whitespace
        count=line.count('\"')
        if count%2==1:
            in_string=True
    if in_string:
        line=line.replace('\r','\r')
        # line already had \n stripped by split, but raw \r remains
        # Ensure any \r is escaped
    else:
        in_string=False
    fixed.append(line)

text='\n'.join(fixed)
try:
    data=json.loads(text)
except Exception as e:
   print(f"Parse error: {e}")
   # Attempt to replace raw \r\n inside strings with JSON escapes
   text=text.replace('\r','')
   data=json.loads(text)

with open('config/lw-tasks.json','w', encoding='utf-8') as f:
    json.dump(data, f, indent=2, ensure_ascii=False)
print('Fixed and saved')
