import requests
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor, as_completed
from tqdm import tqdm
from functools import lru_cache

# 设置HTTP和HTTPS请求的代理服务器。
session = requests.Session()
session.proxies = {
    'http': 'http://127.0.0.1:7897',
    'https': 'http://127.0.0.1:7897',
}

# 使用lru_cache装饰器来缓存查询结果，避免对相同标签进行重复查询。
@lru_cache(maxsize=None)
def get_category(tag_name):
    """获取指定标签的类别。

    函数接收一个标签名作为参数，并返回该标签对应的类别ID。
    """
    try:
        response = session.get(f'https://danbooru.donmai.us/tags.json?search[name]={tag_name}&limit=1')
        response.raise_for_status()  # 检查HTTP请求是否成功
        tags = response.json()

        if tags and isinstance(tags, list) and len(tags) > 0:
            return tags[0].get('category')  # 返回第一个匹配标签的'category'属性值。
        else:
            print(f"No tag found for '{tag_name}'")
            return None
    except requests.exceptions.RequestException as e:
        print(f"Network error occurred while fetching data for '{tag_name}': {e}")
        return None
    except ValueError as e:  # JSON解码失败
        print(f"Invalid JSON response for '{tag_name}': {e}")
        return None
    except Exception as e:
        print(f"An unexpected error occurred for '{tag_name}': {e}")
        return None


def txt_files_to_dict(directory):
    """读取目录中的所有 .txt 文件并转换成字典。

    函数遍历给定目录下的所有文本文件，将每个文件的内容解析为列表，并构建一个字典，
    字典的键是文件名（不包括扩展名），值是文件内容的列表。
    """
    files_content = {}
    path = Path(directory)

    for file_path in path.glob('*.txt'):
        with file_path.open('r', encoding='utf-8') as file:
            elements = [element.strip() for element in file.read().split(',')]
            files_content[file_path.stem] = elements

    return files_content


def filter_and_save_tags(directory, dict_of_files):
    """过滤标签并保存回原文件。

    对于字典中的每个标签列表，函数会并发地检查每个标签的类别，并根据类别决定是否保留标签。
    最后，它将过滤后的标签列表写回到原始文件中。
    """
    path = Path(directory)

    def process_element(element):
        """辅助函数：检查单个元素的类别，并根据类别决定是否保留或修改"""
        category = get_category(element)
        if category == 1:  # 如果标签类别为1，则添加前缀'artist:'
            return f"artist:{element}"
        elif category not in (3, 5):  # 如果标签类别不是3或5，则保留标签。
            return element
        return None  # 如果类别为3或5，则不保留标签。

    for key, elements in tqdm(dict_of_files.items()):
        filtered_elements = []

        # 使用ThreadPoolExecutor并发处理每个元素，提高效率。
        with ThreadPoolExecutor(max_workers=3) as executor:
            future_to_element = {executor.submit(process_element, element): element for element in elements}
            for future in as_completed(future_to_element):
                result = future.result()
                if result is not None:
                    filtered_elements.append(result)

        # 将过滤后的标签列表写回文件。
        file_path = path / f"{key}.txt"
        with file_path.open('w', encoding='utf-8') as file:
            file.write(','.join(filtered_elements))


def main():
    """主函数，设置要处理的目录路径，调用其他函数完成整个流程"""
    directory_path = r'danbooru_images\the_herta'
    dict_of_files = txt_files_to_dict(directory_path)
    filter_and_save_tags(directory_path, dict_of_files)


if __name__ == "__main__":
    main()  # 当脚本被直接运行时，执行main函数。