import os


def escape_parentheses(text):
    result = []
    i = 0
    while i < len(text):
        if text[i] == '(' and (i == 0 or text[i - 1] != '\\'):
            result.append(r'\(')
        elif text[i] == ')' and (i == 0 or text[i - 1] != '\\'):
            result.append(r'\)')
        else:
            result.append(text[i])
        i += 1
    return ''.join(result)


def process_files_in_directory(directory):
    for root, _, files in os.walk(directory):
        for filename in files:
            if filename.endswith('.txt'):
                filepath = os.path.join(root, filename)
                try:
                    with open(filepath, 'r', encoding='utf-8') as file:
                        content = file.read()

                    # 修改内容中的括号
                    modified_content = escape_parentheses(content)

                    # 如果内容有变化，则写回文件
                    if content != modified_content:
                        with open(filepath, 'w', encoding='utf-8') as file:
                            file.write(modified_content)
                        print(f"Successfully processed: {filepath}")
                    else:
                        print(f"No changes needed for: {filepath}")
                except Exception as e:
                    print(f"Failed to process: {filepath}, Error: {e}")


if __name__ == '__main__':
    target_directory = r'danbooru_images\the_herta'  # 替换为你的目标文件夹路径，请使用原始字符串（前缀'r'）来避免反斜杠问题
    process_files_in_directory(target_directory)